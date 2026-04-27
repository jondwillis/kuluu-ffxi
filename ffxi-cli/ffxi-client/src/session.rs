//! Session actor — orchestrates auth → lobby → map bootstrap → zone-in →
//! keepalive, and emits typed `AgentEvent`s for both the JSON sidechannel and
//! (eventually) the TUI to subscribe to.
//!
//! Sequence-number bookkeeping for the map session lives here. The server
//! caches the last bundle it sent and *resends it* if our `sync_in` doesn't
//! match its `server_packet_id` (failure mode #1 in the plan: silent
//! sequence desync). Tracking `server_last_seq` from incoming bundles and
//! using it as the ack on outgoing bundles keeps us walking forward.

use anyhow::{Context, Result, anyhow, bail};
use ffxi_proto::{decode, framing};
use tokio::sync::{broadcast, mpsc};

use crate::auth_client::AuthClient;
use crate::lobby_client::LobbyClient;
use crate::map_client::{self, BootstrapArgs, MapClient};
use crate::state::{
    AgentCommand, AgentEvent, BlowfishStatus, ChatChannel, ChatLine, Diagnostics, Entity,
    EntityKind, Position, Stage, Vec3,
};

#[allow(dead_code)]
const _UNUSED: Option<crate::state::SessionState> = None;

/// Outcome of one map session attempt — `run()`'s outer loop drives the
/// reconnect flow when the server signals a zone change via `0x00B`.
#[derive(Debug)]
enum MapOutcome {
    /// Session ended cleanly (agent disconnect, idle timeout, or terminal
    /// server logout that's not a zone change).
    Disconnected,
    /// Server signaled zone change. Caller should rotate `key3[16..20]` by
    /// +2 (LE u32) and reconnect the UDP socket to `new_addr`, then run a
    /// fresh bootstrap.
    Reconnect { new_addr: std::net::SocketAddr },
}

#[derive(Clone, Debug)]
pub struct Config {
    pub server: String,
    pub map_host_override: Option<String>,
    pub auth_port: u16,
    pub data_port: u16,
    pub view_port: u16,
    pub user: String,
    pub password: String,
    pub char_id: u32,
    pub char_name: String,
}

pub async fn run(
    cfg: Config,
    mut cmd_rx: mpsc::Receiver<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
) -> Result<()> {
    // Phase 1 — auth + lobby (one shot per session).
    emit_stage(&event_tx, Stage::Authenticating);
    let auth = AuthClient::new(cfg.server.clone(), cfg.auth_port);
    auth.ensure_account(&cfg.user, &cfg.password).await.ok();
    let auth_session = auth
        .login(&cfg.user, &cfg.password)
        .await
        .context("auth login")?;
    let cert_sha256 = auth.verifier.fingerprint_hex();

    emit_stage(&event_tx, Stage::LobbyHandshake);
    let lobby = LobbyClient::new(cfg.server.clone(), cfg.data_port, cfg.view_port);
    let mut key3 = [0u8; 20];
    for (i, b) in key3.iter_mut().enumerate() {
        *b = ((i as u8).wrapping_mul(0x37)) ^ 0x5a;
    }
    let handoff = lobby
        .handshake(&auth_session, cfg.char_id, &cfg.char_name, 0, key3)
        .await
        .context("lobby handshake")?;

    let lobby_ip = format!(
        "{}.{}.{}.{}",
        handoff.server_ip & 0xFF,
        (handoff.server_ip >> 8) & 0xFF,
        (handoff.server_ip >> 16) & 0xFF,
        (handoff.server_ip >> 24) & 0xFF,
    );
    let mut server_addr: std::net::SocketAddr = match cfg.map_host_override.as_deref() {
        Some(host) => tokio::net::lookup_host((host, handoff.server_port))
            .await
            .context("resolving map_host_override")?
            .next()
            .ok_or_else(|| anyhow!("no addresses for {host}"))?,
        None => format!("{lobby_ip}:{}", handoff.server_port)
            .parse()
            .context("parsing map server address from lobby")?,
    };

    let bootstrap = BootstrapArgs {
        char_id: cfg.char_id,
        char_name: &cfg.char_name,
        account_name: &cfg.user,
        ticket: auth_session.session_hash,
        version: 0,
        platform: *b"PC\0\0",
        cli_lang: 0,
    };

    // Phase 2 — map session, with reconnect-on-zone-change as the outer loop.
    let mut current_seed = key3;
    let mut iteration: u32 = 0;
    loop {
        iteration += 1;
        let outcome = run_map_session(
            &cfg,
            &auth_session,
            &bootstrap,
            server_addr,
            current_seed,
            cert_sha256.clone(),
            iteration,
            &mut cmd_rx,
            &event_tx,
        )
        .await?;

        match outcome {
            MapOutcome::Disconnected => return Ok(()),
            MapOutcome::Reconnect { new_addr } => {
                let prev_status = BlowfishStatus::PendingZone;
                map_client::rotate_session_key_seed(&mut current_seed);
                let _ = event_tx.send(AgentEvent::KeyRotated {
                    previous_status: prev_status,
                });
                server_addr = new_addr;
                emit_stage(&event_tx, Stage::Zoning);
                // The new map session expects the rotated key in
                // `accounts_sessions.session_key`. The server writes that
                // synchronously inside `send_parse` *after* sending the
                // 0x00B, so by the time we get here the row is already
                // current. Same connect→map ZMQ pacing applies for the
                // pending-session creation.
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_map_session(
    _cfg: &Config,
    _auth_session: &crate::auth_client::AuthSession,
    bootstrap: &BootstrapArgs<'_>,
    server_addr: std::net::SocketAddr,
    seed: [u8; 20],
    cert_sha256: Option<String>,
    iteration: u32,
    cmd_rx: &mut mpsc::Receiver<AgentCommand>,
    event_tx: &broadcast::Sender<AgentEvent>,
) -> Result<MapOutcome> {
    if iteration == 1 {
        emit_stage(event_tx, Stage::MapBootstrap);
        // Same connect→map ZMQ pacing the original bootstrap needed.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    }
    let map = MapClient::connect(server_addr, seed).await?;

    // First bootstrap creates server-side session; second triggers parse +
    // send_parse (the recv_parse pass-by-value PSession trick).
    map.send_bootstrap(bootstrap).await?;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    map.send_bootstrap(bootstrap).await?;

    if iteration == 1 {
        let _ = event_tx.send(AgentEvent::Connected {
            account_id: 0, // filled later when LOGIN response is decoded
            char_id: bootstrap.char_id,
            character: bootstrap.char_name.to_string(),
            zone_id: 0,
        });
    }
    emit_stage(event_tx, Stage::Zoning);

    let flood_deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let mut server_last_seq: u16 = 0;
    let mut total_subs = 0usize;
    let mut pending_event_end: Vec<(u32, u16, u16)> = Vec::new();
    let mut self_act_index: Option<u16> = None;
    while std::time::Instant::now() < flood_deadline {
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            map.recv_decrypted(),
        )
        .await
        {
            Ok(Ok(buf)) => {
                let header = framing::Header::read(&buf[..framing::FFXI_HEADER_SIZE]);
                server_last_seq = header.id_and_size;
                for sub in framing::walk_sub_packets(&buf[framing::FFXI_HEADER_SIZE..]).flatten() {
                    total_subs += 1;
                    handle_sub_packet(
                        &sub,
                        event_tx,
                        &mut pending_event_end,
                        bootstrap.char_id,
                        &mut self_act_index,
                    );
                }
            }
            _ => break,
        }
    }
    tracing::info!(
        iteration,
        total_subs,
        server_last_seq,
        "zone-in flood drained"
    );

    let mut sub_seq: u16 = 2;
    let mut bundle_seq: u16 = 3;
    {
        let mut payload = build_subpacket_netend(sub_seq);
        sub_seq = sub_seq.wrapping_add(1);
        payload.extend(build_subpacket_zone_transition(sub_seq));
        sub_seq = sub_seq.wrapping_add(1);
        map.send_encrypted(&payload, bundle_seq, server_last_seq)
            .await?;
        bundle_seq = bundle_seq.wrapping_add(1);
    }
    emit_stage(event_tx, Stage::InZone);
    let _ = event_tx.send(AgentEvent::Diagnostics {
        diagnostics: Diagnostics {
            stage: Some(Stage::InZone),
            blowfish_status: Some(BlowfishStatus::Accepted),
            sync_in: Some(server_last_seq),
            sync_out: Some(bundle_seq),
            last_server_packet_age_ms: Some(0),
            cert_sha256,
            map_server_addr: Some(server_addr.to_string()),
        },
    });

    keepalive_loop(
        map,
        bundle_seq,
        sub_seq,
        server_last_seq,
        pending_event_end,
        bootstrap.char_id,
        self_act_index,
        cmd_rx,
        event_tx.clone(),
    )
    .await
}

/// Decode a single S2C sub-packet and emit typed `AgentEvent`s. Returns the
/// `(UniqueNo, ActIndex, EventNum)` triple if it's an event-start packet so
/// the caller can queue an auto-dismiss `0x05B EVENT_END`.
///
/// `self_char_id` lets us recognize the player's own CHAR_PC packet during
/// the zone-in flood and stash the player's per-zone `ActIndex` in
/// `self_act_index` — required when sending packets that target the player
/// (e.g. `0x05E` MAPRECT for zone-line transitions).
fn handle_sub_packet(
    sub: &framing::SubPacket<'_>,
    event_tx: &broadcast::Sender<AgentEvent>,
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    self_char_id: u32,
    self_act_index: &mut Option<u16>,
) {
    use ffxi_proto::map::s2c;
    match sub.opcode {
        op if op == s2c::CHAR_PC || op == s2c::CHAR_NPC => {
            if let Ok(head) = decode::PosHead::decode(sub.data) {
                let kind = if op == s2c::CHAR_PC {
                    EntityKind::Pc
                } else {
                    EntityKind::Npc
                };
                if op == s2c::CHAR_PC && head.unique_no == self_char_id {
                    *self_act_index = Some(head.act_index);
                }
                let name = decode::PosHead::try_extract_name(op, sub.data);
                let _ = event_tx.send(AgentEvent::EntityUpserted {
                    entity: Entity {
                        id: head.unique_no,
                        act_index: head.act_index,
                        kind,
                        name,
                        pos: Vec3 {
                            x: head.x,
                            y: head.y,
                            z: head.z,
                        },
                        heading: head.dir,
                        hp_pct: Some(head.hpp),
                        bt_target_id: head.bt_target_id,
                    },
                });
            }
        }
        op if op == s2c::ENTITY_UPDATE1 || op == s2c::ENTITY_UPDATE2 => {
            if let Ok(head) = decode::PosHead::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::EntityUpserted {
                    entity: Entity {
                        id: head.unique_no,
                        act_index: head.act_index,
                        kind: EntityKind::Other,
                        name: None,
                        pos: Vec3 {
                            x: head.x,
                            y: head.y,
                            z: head.z,
                        },
                        heading: head.dir,
                        hp_pct: Some(head.hpp),
                        bt_target_id: head.bt_target_id,
                    },
                });
            }
        }
        op if op == s2c::EVENT || op == s2c::EVENTSTR => {
            // 0x032/0x033 share the same UniqueNo @ [0..4], ActIndex @ [4..6],
            // EventNum @ [6..8] prefix.
            if sub.data.len() >= 8 {
                let unique_no = u32::from_le_bytes(sub.data[0..4].try_into().unwrap());
                let act_index = u16::from_le_bytes(sub.data[4..6].try_into().unwrap());
                let event_num = u16::from_le_bytes(sub.data[6..8].try_into().unwrap());
                let _ = event_tx.send(AgentEvent::EventStart {
                    event_id: ((unique_no as u64) << 16 | event_num as u64) as u32,
                });
                pending_event_end.push((unique_no, act_index, event_num));
            }
        }
        op if op == s2c::EVENTNUM => {
            // 0x034: UniqueNo @ [0..4], num[8] @ [4..36], ActIndex @ [36..38],
            // EventNum @ [38..40].
            if sub.data.len() >= 40 {
                let unique_no = u32::from_le_bytes(sub.data[0..4].try_into().unwrap());
                let act_index = u16::from_le_bytes(sub.data[36..38].try_into().unwrap());
                let event_num = u16::from_le_bytes(sub.data[38..40].try_into().unwrap());
                let _ = event_tx.send(AgentEvent::EventStart {
                    event_id: ((unique_no as u64) << 16 | event_num as u64) as u32,
                });
                pending_event_end.push((unique_no, act_index, event_num));
            }
        }
        op if op == s2c::MESSAGE => {
            if let Ok(text) = std::str::from_utf8(sub.data) {
                let line = ChatLine {
                    channel: ChatChannel::System,
                    sender: "<server>".into(),
                    text: text.trim_end_matches('\0').to_string(),
                    server_ts: 0,
                };
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
        }
        op if op == s2c::GROUP_LIST => {
            if let Ok((attrs, extra)) = decode::PartyAttrs::decode_group_list(sub.data) {
                let _ = event_tx.send(AgentEvent::PartyMemberUpdated {
                    member: party_member_from_attrs(&attrs, Some(&extra)),
                });
            }
        }
        op if op == s2c::GROUP_ATTR => {
            if let Ok(attrs) = decode::PartyAttrs::decode_group_attr(sub.data) {
                let _ = event_tx.send(AgentEvent::PartyMemberUpdated {
                    member: party_member_from_attrs(&attrs, None),
                });
            }
        }
        _ => {
            // Surface unknown opcodes at debug level; not an error.
            tracing::trace!(opcode = format!("0x{:03x}", sub.opcode), len = sub.data.len(), "unhandled sub-packet");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn keepalive_loop(
    map: MapClient,
    mut bundle_seq: u16,
    mut sub_seq: u16,
    mut server_last_seq: u16,
    mut pending_event_end: Vec<(u32, u16, u16)>,
    self_char_id: u32,
    mut self_act_index: Option<u16>,
    cmd_rx: &mut mpsc::Receiver<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
) -> Result<MapOutcome> {
    let mut self_pos = Position::default();
    let mut last_recv = std::time::Instant::now();
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    tick.tick().await;
    let mut reconnect_addr: Option<std::net::SocketAddr> = None;
    let mut terminal_disconnect = false;
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    None => break,
                    Some(AgentCommand::Move { x, y, z, heading }) => {
                        self_pos = Position { pos: Vec3 { x, y, z }, heading };
                        let _ = event_tx.send(AgentEvent::PositionChanged { pos: self_pos });
                    }
                    Some(AgentCommand::StopMove) => { /* keepalive resends current pos */ }
                    Some(AgentCommand::EndEvent) => {
                        // Flush all pending event-ends now (in addition to the next keepalive bundle).
                        if !pending_event_end.is_empty() {
                            let mut payload = Vec::new();
                            for (unique_no, act_index, event_num) in pending_event_end.drain(..) {
                                payload.extend(build_subpacket_event_end(sub_seq, unique_no, act_index, event_num));
                                sub_seq = sub_seq.wrapping_add(1);
                            }
                            if let Err(e) = map.send_encrypted(&payload, bundle_seq, server_last_seq).await {
                                tracing::warn!(error = %e, "EVENT_END send failed");
                            }
                            bundle_seq = bundle_seq.wrapping_add(1);
                            let _ = event_tx.send(AgentEvent::EventEnded);
                        }
                    }
                    Some(AgentCommand::Disconnect) => {
                        let _ = event_tx.send(AgentEvent::Disconnected { reason: "agent requested disconnect".into() });
                        break;
                    }
                    Some(AgentCommand::Snapshot) => {
                        let _ = event_tx.send(AgentEvent::Diagnostics {
                            diagnostics: Diagnostics {
                                stage: Some(Stage::InZone),
                                blowfish_status: Some(BlowfishStatus::Accepted),
                                sync_in: Some(server_last_seq),
                                sync_out: Some(bundle_seq),
                                last_server_packet_age_ms: Some(last_recv.elapsed().as_millis() as u64),
                                cert_sha256: None,
                                map_server_addr: Some(map.server_addr().to_string()),
                            },
                        });
                    }
                    Some(AgentCommand::Chat { kind, text }) => {
                        let payload = build_subpacket_chat(sub_seq, kind, &text);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "chat send failed");
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::Tell { to, text }) => {
                        let payload = build_subpacket_tell(sub_seq, &to, &text);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "tell send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("tell send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::Action {
                        target_id,
                        target_index,
                        kind,
                    }) => {
                        let payload =
                            build_subpacket_action(sub_seq, target_id, target_index, &kind);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "action send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("action send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::Follow { .. })
                    | Some(AgentCommand::Engage { .. })
                    | Some(AgentCommand::PathTo { .. })
                    | Some(AgentCommand::Cancel) => {
                        // These are reactor-handled goal commands. If they
                        // reach the session loop, the reactor middleware
                        // (`crate::reactor::run`) wasn't wired in front —
                        // surface that as an error rather than silently drop.
                        let _ = event_tx.send(AgentEvent::Error {
                            message: "reactor goal command reached session loop \
                                      (reactor middleware not wired)"
                                .into(),
                        });
                    }
                    Some(AgentCommand::RequestZoneChange { line_id }) => {
                        // 0x05E `GP_CLI_COMMAND_MAPRECT` — sent when the player
                        // crosses a zoneline. The server validates that
                        // `PChar->loc.p` is within ~40 yalms of the zoneline's
                        // `originPos` (Phoenix/src/map/packets/c2s/0x05e_maprect.cpp:212),
                        // so the agent must walk into range first; this command
                        // is "I'm at the zoneline, take me through," not a
                        // free-form warp.
                        let Some(act_index) = self_act_index else {
                            let _ = event_tx.send(AgentEvent::Error {
                                message: "RequestZoneChange before self ActIndex \
                                          known (no CHAR_PC for self yet)"
                                    .into(),
                            });
                            continue;
                        };
                        let payload = build_subpacket_maprect(
                            sub_seq,
                            line_id,
                            self_pos.pos.x,
                            self_pos.pos.y,
                            self_pos.pos.z,
                            act_index,
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "MAPRECT send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("MAPRECT send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                }
            }
            _ = tick.tick() => {
                // Build a bundle: any auto-event-ends drained, then a POS keepalive.
                let mut payload = Vec::new();
                for (unique_no, act_index, event_num) in pending_event_end.drain(..) {
                    payload.extend(build_subpacket_event_end(sub_seq, unique_no, act_index, event_num));
                    sub_seq = sub_seq.wrapping_add(1);
                    let _ = event_tx.send(AgentEvent::EventEnded);
                }
                payload.extend(build_subpacket_pos(
                    sub_seq,
                    self_pos.pos.x,
                    self_pos.pos.y,
                    self_pos.pos.z,
                    self_pos.heading,
                ));
                sub_seq = sub_seq.wrapping_add(1);
                if let Err(e) = map.send_encrypted(&payload, bundle_seq, server_last_seq).await {
                    tracing::warn!(error = %e, "keepalive send failed");
                    let _ = event_tx.send(AgentEvent::Error { message: format!("keepalive send: {e}") });
                    break;
                }
                bundle_seq = bundle_seq.wrapping_add(1);
            }
            res = tokio::time::timeout(std::time::Duration::from_millis(50), map.recv_decrypted()) => {
                if let Ok(Ok(buf)) = res {
                    last_recv = std::time::Instant::now();
                    let header = framing::Header::read(&buf[..framing::FFXI_HEADER_SIZE]);
                    server_last_seq = header.id_and_size;
                    for sub in framing::walk_sub_packets(&buf[framing::FFXI_HEADER_SIZE..]).flatten() {
                        if sub.opcode == ffxi_proto::map::s2c::LOGOUT {
                            if let Some(logout) = decode::ServerLogout::decode(sub.data).ok() {
                                if logout.is_zone_change() {
                                    let new_addr = parse_logout_addr(&logout, map.server_addr());
                                    let _ = event_tx.send(AgentEvent::ZoneChanged {
                                        from: None,
                                        to: 0,
                                    });
                                    reconnect_addr = Some(new_addr);
                                } else {
                                    let _ = event_tx.send(AgentEvent::Disconnected {
                                        reason: format!(
                                            "server logout state={}",
                                            logout.logout_state
                                        ),
                                    });
                                    terminal_disconnect = true;
                                }
                            } else {
                                let _ = event_tx.send(AgentEvent::Error {
                                    message: "could not decode 0x00B LOGOUT".into(),
                                });
                                terminal_disconnect = true;
                            }
                        } else {
                            handle_sub_packet(
                                &sub,
                                &event_tx,
                                &mut pending_event_end,
                                self_char_id,
                                &mut self_act_index,
                            );
                        }
                    }
                }
            }
        }

        if reconnect_addr.is_some() || terminal_disconnect {
            break;
        }

        if last_recv.elapsed() > std::time::Duration::from_secs(60) {
            let _ = event_tx.send(AgentEvent::Disconnected {
                reason: "no server packets for 60s".into(),
            });
            break;
        }
    }

    // Drop `map` here so the UDP socket closes before the outer loop
    // potentially binds a new socket to the same kernel port.
    drop(map);

    if let Some(addr) = reconnect_addr {
        Ok(MapOutcome::Reconnect { new_addr: addr })
    } else {
        Ok(MapOutcome::Disconnected)
    }
}

/// Resolve the 0x00B's `Iwasaki` field to a concrete socket. The IP/port the
/// server hands back is the *internal* address from its perspective — when
/// the dev stack runs all zones in one map container, this is the same
/// container's IP. We therefore reuse the current `map.server_addr()`
/// host:port if the new IP/port are zero-ish or match the current.
fn parse_logout_addr(
    logout: &decode::ServerLogout,
    current: std::net::SocketAddr,
) -> std::net::SocketAddr {
    let new_ip = logout.new_server_ip;
    let new_port = logout.new_server_port;
    if new_ip == 0 || new_port == 0 {
        return current;
    }
    let candidate: std::net::SocketAddr = format!(
        "{}.{}.{}.{}:{}",
        new_ip & 0xFF,
        (new_ip >> 8) & 0xFF,
        (new_ip >> 16) & 0xFF,
        (new_ip >> 24) & 0xFF,
        new_port,
    )
    .parse()
    .unwrap_or(current);
    // If the lobby already mapped 127.0.0.1 → docker hostname for us, keep
    // that mapping when only the port changes.
    if candidate.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        && current.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    {
        return std::net::SocketAddr::new(current.ip(), new_port);
    }
    candidate
}

fn emit_stage(tx: &broadcast::Sender<AgentEvent>, stage: Stage) {
    let _ = tx.send(AgentEvent::StageChanged { stage });
}

/// Drain the broadcast event log and fold each event into a watch-published
/// `SessionState`. Lagged events are skipped (not re-driven) — the watch
/// view is "latest snapshot" semantics, so missing intermediate state under
/// load is correct, not lossy.
pub async fn run_event_folder(
    mut event_rx: broadcast::Receiver<AgentEvent>,
    state_tx: tokio::sync::watch::Sender<crate::state::SessionState>,
) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match event_rx.recv().await {
            Ok(event) => state_tx.send_modify(|s| s.apply_event(&event)),
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        }
    }
}

fn build_subpacket_header(opcode: u16, size_words: u16, sync: u16) -> [u8; 4] {
    let id_and_size = opcode | (size_words << 9);
    let mut h = [0u8; 4];
    h[0..2].copy_from_slice(&id_and_size.to_le_bytes());
    h[2..4].copy_from_slice(&sync.to_le_bytes());
    h
}

fn build_subpacket_netend(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x00D, 2, sync));
    buf
}

fn build_subpacket_zone_transition(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x011, 2, sync));
    // PacketValidator on the server demands `unknown00 == 2`. See
    // `server/src/map/packets/c2s/0x011_zone_transition.cpp::validate`. The
    // field is at body offset 0 (= sub-packet offset 4).
    buf[4] = 2;
    buf
}

/// `GP_CLI_COMMAND_CHAT_STD` (0x0B5) — 4-byte header + 1B Kind + 1B padding +
/// up to 128B Str. We round the total body length up to a 4-byte boundary
/// because the sub-packet `size_words` field is in 4-byte units (i.e.
/// `size_words = total_bytes / 4`).
fn build_subpacket_chat(sync: u16, kind: u8, text: &str) -> Vec<u8> {
    // Reserve up to 128 bytes for the string (server's `Str[128]` limit), but
    // pack tightly: 4 hdr + 2 fixed + max 128 = 134 — and round up to mul-of-4.
    let str_bytes = text.as_bytes();
    let str_len = str_bytes.len().min(127); // leave 1 byte for NUL
    let body_unpadded = 2 /* kind+pad */ + str_len + 1 /* NUL */;
    let body_padded = (body_unpadded + 3) & !3;
    let total = 4 + body_padded;
    let size_words = (total / 4) as u16;

    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x0B5, size_words, sync));
    buf[4] = kind;
    // buf[5] = unknown00 (padding) stays 0
    buf[6..6 + str_len].copy_from_slice(&str_bytes[..str_len]);
    // NUL terminator implicit in the zeroed buffer
    buf
}

/// `GP_CLI_COMMAND_CHAT_NAME` (0x0B6) — `/tell`. 4-byte header + 1B
/// unknown00 + 1B unknown01 + 15B sName + variable Mes (≤128). Layout:
/// `Phoenix/src/map/packets/c2s/0x0b6_chat_name.h`. We pack tightly
/// (variable Mes length, NUL-terminated, padded up to a 4-byte
/// boundary) like `build_subpacket_chat` — same `size_words` math.
fn build_subpacket_tell(sync: u16, recipient: &str, text: &str) -> Vec<u8> {
    let r_bytes = recipient.as_bytes();
    let r_len = r_bytes.len().min(14); // leave 1 byte NUL in sName[15]
    let t_bytes = text.as_bytes();
    let t_len = t_bytes.len().min(127); // leave 1 byte NUL in Mes[128]
    // body = 1 unknown00 + 1 unknown01 + 15 sName + variable Mes + 1 NUL
    let body_unpadded = 1 + 1 + 15 + t_len + 1;
    let body_padded = (body_unpadded + 3) & !3;
    let total = 4 + body_padded;
    let size_words = (total / 4) as u16;

    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x0B6, size_words, sync));
    // buf[4] = unknown00 stays 0
    // buf[5] = unknown01 stays 0
    // sName at body offset 2 = absolute 6.
    buf[6..6 + r_len].copy_from_slice(&r_bytes[..r_len]);
    // Mes at body offset 17 = absolute 21. NUL at end implicit (zero-init).
    buf[21..21 + t_len].copy_from_slice(&t_bytes[..t_len]);
    buf
}

/// `GP_CLI_COMMAND_EVENTEND` (0x05B) — 4-byte header + 16-byte body = 20 bytes
/// (size_words=5). We send Mode=0, EndPara=0 — the "skip whatever the NPC was
/// trying to say" form.
fn build_subpacket_event_end(sync: u16, unique_no: u32, act_index: u16, event_num: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 20];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x05B, 5, sync));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    // EndPara u32 stays 0
    buf[12..14].copy_from_slice(&act_index.to_le_bytes());
    // Mode u16 stays 0
    buf[16..18].copy_from_slice(&event_num.to_le_bytes());
    // EventPara u16 stays 0
    buf
}

/// `GP_CLI_COMMAND_ACTION` (0x01A) — 4-byte header + 4 UniqueNo + 2 ActIndex
/// + 2 ActionID + 16 ActionBuf = 28 bytes (size_words=7). The `ActionKind`
/// determines both the wire `ActionID` and the layout of the 16-byte buf
/// (see `ActionKind::fill_action_buf`).
fn build_subpacket_action(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    kind: &crate::state::ActionKind,
) -> Vec<u8> {
    let mut buf = vec![0u8; 28];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x01A, 7, sync));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..10].copy_from_slice(&act_index.to_le_bytes());
    buf[10..12].copy_from_slice(&kind.action_id().to_le_bytes());
    let mut action_buf = [0u8; 16];
    kind.fill_action_buf(&mut action_buf);
    buf[12..28].copy_from_slice(&action_buf);
    buf
}

/// Convert a `decode::PartyAttrs` (+ optional list-only extras) into the
/// state-layer `PartyMember` the agent sees. When `extra` is `None` (we're
/// processing a `0x0DF GROUP_ATTR` for self / Trust), `name` and the
/// leader flags are left empty — `apply_event` preserves whatever the
/// previous `0x0DD` set.
fn party_member_from_attrs(
    attrs: &decode::PartyAttrs,
    extra: Option<&decode::PartyListExtra>,
) -> crate::state::PartyMember {
    crate::state::PartyMember {
        id: attrs.unique_no,
        act_index: attrs.act_index,
        name: extra.and_then(|e| e.name.clone()),
        hp: attrs.hp,
        mp: attrs.mp,
        tp: attrs.tp,
        hp_pct: attrs.hpp,
        mp_pct: attrs.mpp,
        zone_no: attrs.zone_no,
        main_job: attrs.mjob_no,
        main_job_lv: attrs.mjob_lv,
        sub_job: attrs.sjob_no,
        sub_job_lv: attrs.sjob_lv,
        is_party_leader: extra.map(|e| e.is_party_leader).unwrap_or(false),
        is_alliance_leader: extra.map(|e| e.is_alliance_leader).unwrap_or(false),
    }
}

/// `GP_CLI_COMMAND_MAPRECT` (0x05E) — 4-byte header + RectID(4) + x/y/z(12)
/// + ActIndex(2) + MyRoomExitBit(1) + MyRoomExitMode(1) = 24 bytes
/// (size_words=6). Wire field order is (x, y, z) — note this differs from
/// 0x015 POS (x, z, y).
///
/// `MyRoomExitBit`/`MyRoomExitMode` are zeroed for non-Mog-House zonelines;
/// the server only consults them when `RectID` is the universal Mog House
/// exit tag (`"zmrq"`) per Phoenix/src/map/packets/c2s/0x05e_maprect.cpp:71.
fn build_subpacket_maprect(
    sync: u16,
    rect_id: u32,
    x: f32,
    y: f32,
    z: f32,
    act_index: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 24];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x05E, 6, sync));
    buf[4..8].copy_from_slice(&rect_id.to_le_bytes());
    buf[8..12].copy_from_slice(&x.to_le_bytes());
    buf[12..16].copy_from_slice(&y.to_le_bytes());
    buf[16..20].copy_from_slice(&z.to_le_bytes());
    buf[20..22].copy_from_slice(&act_index.to_le_bytes());
    // buf[22] = MyRoomExitBit, buf[23] = MyRoomExitMode — zero for
    // non-Mog-House zonelines.
    buf
}

fn build_subpacket_pos(sync: u16, x: f32, y: f32, z: f32, heading: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 32];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x015, 8, sync));
    buf[4..8].copy_from_slice(&x.to_le_bytes());
    buf[8..12].copy_from_slice(&z.to_le_bytes());
    buf[12..16].copy_from_slice(&y.to_le_bytes());
    buf[20] = heading;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    buf[24..28].copy_from_slice(&now.to_le_bytes());
    buf
}

#[allow(dead_code)]
fn _hint() -> Result<()> {
    bail!("compile guard")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tell_packet_layout_matches_phoenix_struct() {
        // Build a /tell to "Vanari" with body "hi". Phoenix layout:
        //   header(4) + unknown00(1) + unknown01(1) + sName[15] + Mes[…] + NUL
        // Body unpadded = 1 + 1 + 15 + 2 + 1 = 20 → padded to 20 (mul-of-4) →
        // total 24 → size_words = 6.
        let buf = build_subpacket_tell(0xABCD, "Vanari", "hi");
        assert_eq!(buf.len(), 24, "total = 4 hdr + 20 body, padded to mul-of-4");

        // Header: opcode 0x0B6 in low 9 bits, size_words=6 in next 7,
        // sync=0xABCD in next 16. id_and_size = 0x0B6 | (6 << 9) = 0x0CB6.
        let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(id_and_size & 0x1FF, 0x0B6, "opcode");
        assert_eq!(id_and_size >> 9, 6, "size_words");
        let sync = u16::from_le_bytes([buf[2], buf[3]]);
        assert_eq!(sync, 0xABCD, "sync passed through");

        // Body field offsets:
        //   buf[4] = unknown00 = 0
        //   buf[5] = unknown01 = 0
        //   buf[6..21] = sName[15], NUL-padded: "Vanari" + 9 NULs
        //   buf[21..23] = Mes "hi"
        //   buf[23] = NUL terminator
        assert_eq!(buf[4], 0, "unknown00");
        assert_eq!(buf[5], 0, "unknown01");
        assert_eq!(&buf[6..12], b"Vanari", "recipient name");
        assert!(buf[12..21].iter().all(|&b| b == 0), "sName NUL-padded");
        assert_eq!(&buf[21..23], b"hi", "message body");
        assert_eq!(buf[23], 0, "trailing NUL");
    }

    #[test]
    fn tell_packet_truncates_oversize_inputs() {
        // Recipient longer than 14 chars (sName[15] - NUL) is truncated.
        let long_name = "a".repeat(50);
        let buf = build_subpacket_tell(0, &long_name, "x");
        // sName starts at body offset 2 = absolute 6, occupies 15 bytes,
        // last byte must be NUL.
        assert_eq!(&buf[6..20], &[b'a'; 14][..], "first 14 chars of name");
        assert_eq!(buf[20], 0, "sName NUL-terminated even on truncation");
    }
}
