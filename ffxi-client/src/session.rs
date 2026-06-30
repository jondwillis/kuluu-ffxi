use anyhow::{anyhow, bail, Context, Result};
use ffxi_proto::{decode, framing};
use tokio::sync::{broadcast, mpsc};

use crate::auth_client::AuthClient;
use crate::lobby_client::LobbyClient;
use crate::map_client::{self, BootstrapArgs, MapClient};
use crate::state::{
    AgentCommand, AgentEvent, BlowfishStatus, ChatChannel, ChatLine, Diagnostics, Entity,
    EntityKind, HealMode, InventoryUpdate, ItemSlot, Position, ShopItem, ShopState, Stage, Vec3,
};

#[allow(dead_code)]
const _UNUSED: Option<crate::state::SessionState> = None;

struct NpcNameResolver {
    root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,
    current: Option<ffxi_dat::NpcNameTable>,
}

impl NpcNameResolver {
    fn new(root: Option<std::sync::Arc<ffxi_dat::DatRoot>>) -> Self {
        Self {
            root,
            current: None,
        }
    }

    fn lookup(&mut self, npc_id: u32) -> Option<&str> {
        let root = self.root.as_ref()?;
        let (zone, _slot) = ffxi_dat::split_id(npc_id)?;

        let zone_matches = self.current.as_ref().is_some_and(|t| t.zone_id() == zone);
        if !zone_matches {
            self.current = match ffxi_dat::NpcNameTable::open(root, zone) {
                Ok(table) => Some(table),
                Err(err) => {
                    tracing::debug!(zone, error = %err, "no NPC-name DAT for zone");
                    None
                }
            };
        }
        self.current.as_ref()?.lookup_by_id(npc_id)
    }
}

#[derive(Clone, Debug)]
pub enum CharSelection {
    Id(u32),
    Name(String),
}

#[derive(Debug)]
enum MapOutcome {
    Disconnected,

    Reconnect {
        new_addr: std::net::SocketAddr,
        via_zoneline: Option<u32>,
    },
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
    pub char_selection: CharSelection,

    pub initial_state: Option<InitialState>,

    pub user_driven_events: bool,

    pub dat_root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,
}

#[derive(Clone, Debug)]
pub struct InitialState {
    pub auth: crate::auth_client::AuthSession,
    pub handoff: crate::lobby_client::MapHandoff,
    pub key3: [u8; 20],
}

pub async fn run(
    cfg: Config,
    mut cmd_rx: mpsc::Receiver<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
) -> Result<()> {
    let auth = AuthClient::new(cfg.server.clone(), cfg.auth_port);
    let cert_sha256 = auth.verifier.fingerprint_hex();

    let (auth_session, handoff, key3, resolved_char_id) = match cfg.initial_state.clone() {
        Some(state) => {
            let char_id = match &cfg.char_selection {
                CharSelection::Id(id) => *id,
                CharSelection::Name(_) => state.handoff.char_id,
            };
            (state.auth, state.handoff, state.key3, char_id)
        }
        None => {
            emit_stage(&event_tx, Stage::Authenticating);
            auth.ensure_account(&cfg.user, &cfg.password).await.ok();
            let auth_session = auth
                .login(&cfg.user, &cfg.password)
                .await
                .context("auth login")?;

            emit_stage(&event_tx, Stage::LobbyHandshake);
            let lobby = LobbyClient::new(cfg.server.clone(), cfg.data_port, cfg.view_port);
            let mut key3 = [0u8; 20];
            for (i, b) in key3.iter_mut().enumerate() {
                *b = ((i as u8).wrapping_mul(0x37)) ^ 0x5a;
            }
            let (char_id, handoff) = match &cfg.char_selection {
                CharSelection::Id(cid) => {
                    let handoff = lobby
                        .handshake(&auth_session, *cid, "", 0, key3)
                        .await
                        .context("lobby handshake")?;
                    (*cid, handoff)
                }
                CharSelection::Name(name) => {
                    let (char_id, handoff) = lobby
                        .handshake_by_name(&auth_session, name, key3)
                        .await
                        .context("lobby handshake by name")?;
                    (char_id, handoff)
                }
            };
            (auth_session, handoff, key3, char_id)
        }
    };

    let lobby_ip = format!(
        "{}.{}.{}.{}",
        handoff.server_ip & 0xFF,
        (handoff.server_ip >> 8) & 0xFF,
        (handoff.server_ip >> 16) & 0xFF,
        (handoff.server_ip >> 24) & 0xFF,
    );
    let server_addr: std::net::SocketAddr = match cfg.map_host_override.as_deref() {
        Some(host) => tokio::net::lookup_host((host, handoff.server_port))
            .await
            .context("resolving map_host_override")?
            .next()
            .ok_or_else(|| anyhow!("no addresses for {host}"))?,
        None => format!("{lobby_ip}:{}", handoff.server_port)
            .parse()
            .context("parsing map server address from lobby")?,
    };

    let char_name_for_bootstrap = match &cfg.char_selection {
        CharSelection::Id(_) => &handoff.character_name,
        CharSelection::Name(n) => n,
    };
    let bootstrap = BootstrapArgs {
        char_id: resolved_char_id,
        char_name: char_name_for_bootstrap,
        account_name: &cfg.user,
        ticket: auth_session.session_hash,
        version: 0,
        platform: *b"PC\0\0",
        cli_lang: 0,
    };

    let mut current_seed = key3;
    let mut iteration: u32 = 0;

    let mut spawn_fallback: Option<Vec3> = None;
    emit_stage(&event_tx, Stage::MapBootstrap);
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let mut map = MapClient::connect(server_addr, current_seed).await?;
    loop {
        iteration += 1;
        let outcome = run_map_session(
            &cfg,
            &auth_session,
            &bootstrap,
            &mut map,
            cert_sha256.clone(),
            iteration,
            spawn_fallback.take(),
            &mut cmd_rx,
            &event_tx,
        )
        .await?;

        match outcome {
            MapOutcome::Disconnected => return Ok(()),
            MapOutcome::Reconnect {
                new_addr,
                via_zoneline,
            } => {
                spawn_fallback = via_zoneline
                    .and_then(ffxi_nav::to_pos_for_line)
                    .map(|p| Vec3 {
                        x: p[0],
                        y: p[1],
                        z: p[2],
                    });
                let prev_status = BlowfishStatus::PendingZone;
                map_client::rotate_session_key_seed(&mut current_seed);
                let _ = event_tx.send(AgentEvent::KeyRotated {
                    previous_status: prev_status,
                });

                let target = match cfg.map_host_override.as_deref() {
                    Some(host) => tokio::net::lookup_host((host, new_addr.port()))
                        .await
                        .context("resolving map_host_override on reconnect")?
                        .next()
                        .ok_or_else(|| anyhow!("no addresses for {host} on reconnect"))?,
                    None => new_addr,
                };
                tracing::info!(
                    reconnect_addr = %target,
                    server_reported = %new_addr,
                    "reconnecting to new map server after zone change"
                );

                map.retarget(target, current_seed);
                emit_stage(&event_tx, Stage::Zoning);

                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_map_session(
    cfg: &Config,
    _auth_session: &crate::auth_client::AuthSession,
    bootstrap: &BootstrapArgs<'_>,
    map: &mut MapClient,
    cert_sha256: Option<String>,
    iteration: u32,

    spawn_fallback: Option<Vec3>,
    cmd_rx: &mut mpsc::Receiver<AgentCommand>,
    event_tx: &broadcast::Sender<AgentEvent>,
) -> Result<MapOutcome> {
    map.send_bootstrap(bootstrap).await?;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    map.send_bootstrap(bootstrap).await?;

    if iteration == 1 {
        let _ = event_tx.send(AgentEvent::Connected {
            account_id: 0,
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
    let mut name_cache: std::collections::HashMap<u32, String> = Default::default();

    let mut kind_cache: std::collections::HashMap<u32, crate::state::EntityKind> =
        Default::default();

    let mut claim_cache: std::collections::HashMap<u32, u32> = Default::default();

    let mut npc_name_resolver = NpcNameResolver::new(cfg.dat_root.clone());

    let mut name_miss_dedup: std::collections::HashMap<
        (u32, crate::state::NameMissKind),
        std::time::Instant,
    > = Default::default();
    let mut current_zone_id: u16 = 0;

    let mut self_pos = Position::default();

    let mut self_pos_seeded = false;

    let mut flood_in_mog_house = false;
    while std::time::Instant::now() < flood_deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(500), map.recv_decrypted())
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
                        bootstrap.char_name,
                        &mut self_act_index,
                        &mut name_cache,
                        &mut kind_cache,
                        &mut claim_cache,
                        &mut name_miss_dedup,
                        &mut current_zone_id,
                        &mut self_pos,
                        &mut self_pos_seeded,
                        &mut npc_name_resolver,
                        &mut flood_in_mog_house,
                        spawn_fallback,
                    );
                }
            }

            Ok(Err(_)) => break,

            Err(_elapsed) => {
                if should_break_flood(self_pos_seeded) {
                    break;
                }
            }
        }
    }
    tracing::info!(
        iteration,
        total_subs,
        server_last_seq,
        self_pos_seeded,
        "zone-in flood drained"
    );
    if !self_pos_seeded {
        tracing::warn!(
            iteration,
            current_zone_id,
            "zone-in flood ended without a self-position seed (no 0x00A LOGIN \
             for self before deadline) — outbound POS suppressed until a \
             CHAR_PC for self lands"
        );
    }

    let mut sub_seq: u16 = 2;
    let mut bundle_seq: u16 = 3;

    {
        let payload = build_subpacket_gameok(sub_seq);
        sub_seq = sub_seq.wrapping_add(1);
        map.send_encrypted(&payload, bundle_seq, server_last_seq)
            .await?;
        bundle_seq = bundle_seq.wrapping_add(1);
        tracing::info!(bundle_seq, sub_seq, "sent 0x00C GAMEOK (zone-in)");
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
            map_server_addr: Some(map.server_addr().to_string()),
        },
    });

    keepalive_loop(
        map,
        bundle_seq,
        sub_seq,
        server_last_seq,
        pending_event_end,
        bootstrap.char_id,
        bootstrap.char_name.to_string(),
        current_zone_id,
        self_act_index,
        cmd_rx,
        event_tx.clone(),
        cfg.user_driven_events,
        name_cache,
        kind_cache,
        claim_cache,
        name_miss_dedup,
        self_pos,
        self_pos_seeded,
        npc_name_resolver,
    )
    .await
}

fn classify_char_npc(
    look_size: Option<u16>,
    act_index: u16,
    owned_by_pc: bool,
    monster_flag: bool,
) -> EntityKind {
    let dynamic_targid = (0x700..=0x8FF).contains(&act_index);
    match look_size {
        // Standard monster meshes split mob/NPC the same way the retail client
        // does: Flags1.MonsterFlag (see research/XiPackets .../0x000E). LSB has
        // no literal flag — vendor/server/src/map/packets/entity_update.cpp writes
        // the STATUS_TYPE enum into that byte, so the bit reads set for mobs
        // (allegiance MOB spawn as STATUS_TYPE::UPDATE) and clear for NPCs
        // (npc_list status NORMAL) and other players (forced NORMAL).
        Some(0) | Some(5) | Some(6) => {
            if owned_by_pc {
                EntityKind::Pet
            } else if monster_flag || dynamic_targid {
                EntityKind::Mob
            } else {
                EntityKind::Npc
            }
        }
        Some(1) | Some(7) => EntityKind::Npc,
        Some(2) | Some(3) | Some(4) => EntityKind::Other,

        _ => EntityKind::Other,
    }
}

fn handle_sub_packet(
    sub: &framing::SubPacket<'_>,
    event_tx: &broadcast::Sender<AgentEvent>,
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    self_char_id: u32,

    self_char_name: &str,
    self_act_index: &mut Option<u16>,
    name_cache: &mut std::collections::HashMap<u32, String>,

    kind_cache: &mut std::collections::HashMap<u32, crate::state::EntityKind>,

    claim_cache: &mut std::collections::HashMap<u32, u32>,

    name_miss_dedup: &mut std::collections::HashMap<
        (u32, crate::state::NameMissKind),
        std::time::Instant,
    >,

    current_zone_id: &mut u16,

    self_pos: &mut Position,

    self_pos_seeded: &mut bool,

    npc_name_resolver: &mut NpcNameResolver,

    was_in_mog_house: &mut bool,

    zoneline_spawn_fallback: Option<Vec3>,
) {
    use ffxi_proto::map::s2c;
    match sub.opcode {
        op if op == s2c::LOGIN => {
            let decoded = decode::ServerLogin::decode(sub.data);
            if let Err(ref e) = decoded {
                tracing::warn!(
                    error = %e,
                    body_len = sub.data.len(),
                    "0x00A LOGIN decode failed — self_pos will stay at default until CHAR_PC"
                );
            }
            if let Ok(login) = decoded {
                *current_zone_id = login.zone_no;
                let head = login.pos_head;

                let _ = event_tx.send(AgentEvent::ZoneChanged {
                    from: None,
                    to: login.zone_no,
                });

                if let Some(game_time) = login.game_time {
                    let _ = event_tx.send(AgentEvent::VanaTimeSynced { game_time });
                }

                if let Some(music) = login.music_num {
                    for (slot, track_id) in music.iter().enumerate() {
                        tracing::info!(slot, track_id, "LOGIN MusicNum");
                        let _ = event_tx.send(AgentEvent::MusicChanged {
                            slot: slot as u8,
                            track_id: *track_id,
                        });
                    }
                }
                if login.unique_no != self_char_id {
                    tracing::warn!(
                        login_unique_no = login.unique_no,
                        self_char_id,
                        zone_no = login.zone_no,
                        pos = format!("({:.1},{:.1},{:.1})", head.x, head.y, head.z),
                        "0x00A LOGIN unique_no != self_char_id — self_pos seed skipped (will spawn at origin until CHAR_PC for self lands)"
                    );
                }
                if login.unique_no == self_char_id {
                    *self_act_index = Some(login.act_index);

                    kind_cache.insert(login.unique_no, EntityKind::Pc);

                    let raw_pos = Vec3 {
                        x: head.x,
                        y: head.y,
                        z: head.z,
                    };
                    let seed_pos = apply_zoneline_spawn_fallback(raw_pos, zoneline_spawn_fallback);
                    *self_pos = Position {
                        pos: seed_pos,
                        heading: head.dir,
                        speed: head.speed,
                        speed_base: head.speed_base,
                    };

                    *self_pos_seeded = true;

                    tracing::info!(
                        unique_no = login.unique_no,
                        self_char_id,
                        zone_no = login.zone_no,
                        pos = format!("({:.1},{:.1},{:.1})", seed_pos.x, seed_pos.y, seed_pos.z),
                        raw_pos = format!("({:.1},{:.1},{:.1})", raw_pos.x, raw_pos.y, raw_pos.z),
                        fallback_applied = seed_pos != raw_pos,
                        heading = head.dir,
                        "self_pos seeded from 0x00A LOGIN"
                    );

                    let _ = event_tx.send(AgentEvent::EntityUpserted {
                        entity: Entity {
                            id: head.unique_no,
                            act_index: head.act_index,
                            kind: EntityKind::Pc,

                            name: Some(self_char_name.to_string()),
                            pos: seed_pos,
                            heading: head.dir,
                            hp_pct: Some(head.hpp),
                            bt_target_id: head.bt_target_id,
                            claim_id: 0,
                            speed: head.speed,
                            speed_base: head.speed_base,

                            look: None,
                            npc_state: None,
                            status: 0,
                        },

                        pos_present: true,
                    });

                    let _ = event_tx.send(AgentEvent::PositionChanged { pos: *self_pos });
                }
            }
        }
        op if op == s2c::CHAR_PC || op == s2c::CHAR_NPC => {
            if let Ok(head) = decode::PosHead::decode(sub.data) {
                if decode::PosHead::is_entity_despawn(op, sub.data) {
                    claim_cache.remove(&head.unique_no);
                    let _ = event_tx.send(AgentEvent::EntityRemoved { id: head.unique_no });

                    return;
                }
                let kind = if op == s2c::CHAR_PC {
                    EntityKind::Pc
                } else {
                    const LOOK_SIZE_OFFSET: usize = 0x2C;
                    // LSB byte 0x20: STATUS_TYPE enum, read by the client as
                    // Flags1.MonsterFlag (bit 0). Always written, so it is
                    // present even on position-only ticks.
                    const MONSTER_FLAG_OFFSET: usize = 0x1C;
                    let look_size = sub
                        .data
                        .get(LOOK_SIZE_OFFSET..LOOK_SIZE_OFFSET + 2)
                        .map(|s| u16::from_le_bytes([s[0], s[1]]));
                    let owned_by_pc = head.send_flag & 0x04 != 0
                        && (sub.data.get(35).copied().unwrap_or(0) & 0x08) != 0;
                    let monster_flag =
                        sub.data.get(MONSTER_FLAG_OFFSET).copied().unwrap_or(0) & 0x01 != 0;
                    let kind =
                        classify_char_npc(look_size, head.act_index, owned_by_pc, monster_flag);
                    if matches!(look_size, Some(0) | Some(5) | Some(6)) {
                        tracing::debug!(
                            target: "entity_classify",
                            id = head.unique_no,
                            monster_flag,
                            status = sub.data.get(MONSTER_FLAG_OFFSET).copied().unwrap_or(0),
                            ?kind,
                            "CHAR_NPC standard-model classify"
                        );
                    }
                    kind
                };

                kind_cache
                    .entry(head.unique_no)
                    .and_modify(|existing| {
                        if !matches!(kind, EntityKind::Other) {
                            *existing = kind;
                        }
                    })
                    .or_insert(kind);
                if op == s2c::CHAR_PC && head.unique_no == self_char_id {
                    *self_act_index = Some(head.act_index);

                    let raw_pos = Vec3 {
                        x: head.x,
                        y: head.y,
                        z: head.z,
                    };
                    let seed_pos = apply_zoneline_spawn_fallback(raw_pos, zoneline_spawn_fallback);
                    *self_pos = Position {
                        pos: seed_pos,
                        heading: head.dir,
                        ..*self_pos
                    };

                    *self_pos_seeded = true;

                    tracing::info!(
                        unique_no = head.unique_no,
                        self_char_id,
                        send_flag = format!("0x{:02x}", head.send_flag),
                        fallback_applied = seed_pos != raw_pos,
                        pos = format!("({:.1},{:.1},{:.1})", seed_pos.x, seed_pos.y, seed_pos.z),
                        heading = head.dir,
                        "self_pos seeded from CHAR_PC for self"
                    );
                }
                let wire_name = decode::PosHead::try_extract_name(op, sub.data);
                if wire_name.is_none() {
                    record_name_miss(
                        op,
                        head.unique_no,
                        head.act_index,
                        sub.data,
                        name_miss_dedup,
                        event_tx,
                    );
                }

                let name = wire_name.or_else(|| {
                    if op == s2c::CHAR_NPC {
                        npc_name_resolver.lookup(head.unique_no).map(str::to_string)
                    } else {
                        None
                    }
                });

                let name = name.map(|n| n.replace('_', " "));
                if let Some(n) = name.as_ref() {
                    if !n.is_empty() {
                        name_cache.insert(head.unique_no, n.clone());
                    }
                }

                const UPDATE_STATUS: u8 = 0x02;
                let (claim_id, bt_target_id) = if op == s2c::CHAR_NPC {
                    let carries_status = sub.data.get(6).copied().unwrap_or(0) & UPDATE_STATUS != 0;
                    let claim = if carries_status {
                        claim_cache.insert(head.unique_no, head.bt_target_id);
                        head.bt_target_id
                    } else {
                        claim_cache.get(&head.unique_no).copied().unwrap_or(0)
                    };
                    (claim, claim)
                } else {
                    (0, head.bt_target_id)
                };

                let send_flag = sub.data.get(6).copied().unwrap_or(0);
                let hp_pct = (send_flag & 0x04 != 0).then_some(head.hpp);

                let look = if op == s2c::CHAR_NPC {
                    decode::LookData::decode_char_npc(sub.data)
                } else if op == s2c::CHAR_PC {
                    decode::LookData::decode_char_pc(sub.data)
                } else {
                    None
                };

                const UPDATE_HP: u8 = 0x04;
                let npc_state = (send_flag & UPDATE_HP != 0)
                    .then(|| match op {
                        s2c::CHAR_NPC => decode::NpcState::decode_char_npc(sub.data),
                        s2c::CHAR_PC => decode::NpcState::decode_char_pc(sub.data),
                        _ => None,
                    })
                    .flatten();

                let status = match op {
                    s2c::CHAR_NPC => decode::NpcState::decode_char_npc_status(sub.data),
                    _ => Some(0),
                }
                .unwrap_or(0);

                if op == s2c::CHAR_PC && head.unique_no == self_char_id && look.is_none() {
                    let start = decode::LookData::CHAR_PC_GRAP_OFFSET;
                    let end = (start + 18).min(sub.data.len());
                    let slice = sub.data.get(start..end).unwrap_or(&[]);
                    let hex: String = slice
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    tracing::info!(
                        target: "self_look_probe",
                        body_len = sub.data.len(),
                        send_flag = sub.data.get(6).copied().unwrap_or(0),
                        grap_hex = %hex,
                        "CHAR_PC for self: look decoded None (body[0x44..0x56] dumped)"
                    );
                }

                const UPDATE_POS: u8 = 0x01;
                let pos_present = send_flag & UPDATE_POS != 0;
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
                        hp_pct,
                        bt_target_id,
                        claim_id,
                        speed: head.speed,
                        speed_base: head.speed_base,
                        look,
                        npc_state,
                        status,
                    },
                    pos_present,
                });
            }
        }
        op if op == s2c::ENTITY_UPDATE1 => match sub.data.first().copied() {
            Some(decode::EntitySetName::SUB_TYPE) => {
                if let Ok(ent) = decode::EntitySetName::decode(sub.data) {
                    if let Some(name) = ent.name {
                        let _ = event_tx.send(AgentEvent::EntityPatched {
                            id: Some(ent.id),
                            act_index: Some(ent.targid),
                            name: Some(name),

                            kind: None,
                            hp_pct: None,
                        });
                    }
                }
            }
            Some(decode::CharSync::SUB_TYPE) => {
                let _ = decode::CharSync::decode(sub.data);
            }
            _ => {}
        },
        op if op == s2c::ENTITY_UPDATE2 => {
            if let Ok(pet) = decode::PetSync::decode(sub.data) {
                if pet.pet_targid != 0 {
                    let _ = event_tx.send(AgentEvent::EntityPatched {
                        id: None,
                        act_index: Some(pet.pet_targid),
                        name: pet.name,
                        kind: Some(EntityKind::Pet),
                        hp_pct: Some(pet.hp_pct),
                    });
                }
            }
        }
        op if op == s2c::BATTLE_MESSAGE => {
            if let Some(line) = decode_battle_message(sub.data, name_cache, kind_cache, true) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
            emit_battle_message_audio_event(sub.data, true, event_tx);
        }
        op if op == s2c::BATTLE_MESSAGE2 => {
            if let Some(line) = decode_battle_message(sub.data, name_cache, kind_cache, false) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
            emit_battle_message_audio_event(sub.data, false, event_tx);
        }
        op if op == s2c::SHOP_LIST => {
            if let Some(shop) = decode_shop_list(sub.data) {
                let _ = event_tx.send(AgentEvent::ShopUpdated { shop });
            }
        }
        op if op == s2c::SHOP_OPEN => {}
        op if op == s2c::BATTLE2 => {
            if let Some((actor_id, action_id, action_kind)) = decode_battle2_header(sub.data) {
                let _ = event_tx.send(AgentEvent::ActionStarted {
                    actor_id,
                    action_id,
                    action_kind,
                });
            }
            for line in decode_battle2_action(sub.data, name_cache, kind_cache) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
        }
        op if op == s2c::MUSIC => {
            if sub.data.len() >= 4 {
                let slot = u16::from_le_bytes([sub.data[0], sub.data[1]]) as u8;
                let track_id = u16::from_le_bytes([sub.data[2], sub.data[3]]);
                tracing::info!(slot, track_id, "0x05F MUSIC packet");
                let _ = event_tx.send(AgentEvent::MusicChanged { slot, track_id });
            }
        }
        op if op == s2c::MUSIC_VOLUME => {
            if sub.data.len() >= 4 {
                let slot = u16::from_le_bytes([sub.data[0], sub.data[1]]) as u8;
                let volume = u16::from_le_bytes([sub.data[2], sub.data[3]]) as u8;
                tracing::info!(slot, volume, "0x060 MUSIC_VOLUME packet");
                let _ = event_tx.send(AgentEvent::MusicVolumeChanged { slot, volume });
            }
        }
        op if op == s2c::CHAR_STATUS => {
            if let Ok(cs) = decode::CharStatus::decode(sub.data) {
                if cs.unique_no == self_char_id {
                    let _ = event_tx.send(AgentEvent::DeathTimerUpdated {
                        seconds_until_homepoint: (cs.hpp == 0)
                            .then(|| cs.seconds_until_homepoint()),
                    });
                    // The server's animation byte carries our fishing macro-state. A fresh
                    // FISHING_START also brings the hook delay; feed both to the machine.
                    if cs.server_status == decode::animation::FISHING_START {
                        let _ = event_tx.send(AgentEvent::FishingCast {
                            hook_delay: cs.fishing_timer,
                        });
                    }
                    let _ = event_tx.send(AgentEvent::FishingServerPhase {
                        phase: decode::animation::fishing_phase(cs.server_status),
                    });
                }
            }
        }
        op if op == s2c::FISH => {
            if let Ok(f) = decode::FishPacket::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::FishHooked { params: f.into() });
            }
        }
        op if op == s2c::EVENTUCOFF => {
            // Mode (u32) sits right after the 4-byte sub-header; the high bits may carry an
            // event id, so match the low byte. Fishing release = a rejected cast (no rod /
            // bait / fishing spot) or the end of fishing.
            // vendor/server/src/map/packets/s2c/0x052_eventucoff.h
            if sub.data.len() >= 4 {
                let mode = u32::from_le_bytes([sub.data[0], sub.data[1], sub.data[2], sub.data[3]]);
                if mode & 0xFF == ffxi_proto::map::eventucoff_mode::FISHING {
                    let _ = event_tx.send(AgentEvent::FishingEnded);
                }
            }
        }
        op if op == s2c::WPOS || op == s2c::WPOS2 => {
            if let Ok(fm) = decode::ForcedMove::decode(sub.data) {
                if fm.unique_no == self_char_id && fm.mode.carries_position() {
                    *self_pos = Position {
                        pos: Vec3 {
                            x: fm.x,
                            y: fm.y,
                            z: fm.z,
                        },
                        heading: fm.heading,
                        ..*self_pos
                    };

                    let duration_ms = 1000u32;
                    let _ = event_tx.send(AgentEvent::ForcedMove {
                        mode: fm.raw_mode,
                        target: *self_pos,
                        duration_ms,
                    });
                }
            }
        }
        op if op == s2c::WEATHER => {
            if let Ok(w) = decode::WeatherPacket::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::WeatherUpdated {
                    weather_number: w.weather_number,
                });
            }
        }
        op if op == s2c::MISCDATA => {
            if let Some((icons, expiries)) = decode_miscdata_status_icons(sub.data) {
                let _ = event_tx.send(AgentEvent::StatusIconsUpdated { icons, expiries });
            }
        }
        op if op == s2c::ABIL_RECAST => {
            let recasts = decode_abil_recast(sub.data);
            let _ = event_tx.send(AgentEvent::AbilityRecastsUpdated { recasts });
        }
        op if op == s2c::SCENARIO_ITEM => {
            if let Ok(ki) = decode::ScenarioItem::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::KeyItemsUpdated {
                    table_index: ki.table_index,
                    ids: ki.owned_key_item_ids(),
                });
            }
        }
        op if op == s2c::EVENT => {
            if let Some(dialog) = decode_event_0x032(sub.data) {
                emit_event_dialog(event_tx, &dialog, pending_event_end, name_cache);
            }
        }
        op if op == s2c::EVENTSTR => {
            if let Some(dialog) = decode_event_0x033(sub.data) {
                emit_event_dialog(event_tx, &dialog, pending_event_end, name_cache);
            }
        }
        op if op == s2c::EVENTNUM => {
            if let Some(dialog) = decode_event_0x034(sub.data) {
                emit_event_dialog(event_tx, &dialog, pending_event_end, name_cache);
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
        op if op == s2c::CHAT => {
            if let Some(line) = decode_chat_std(sub.data) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
        }
        op if op == s2c::SYSTEMMES => {
            if let Ok(m) = decode::SystemMessage::decode(sub.data) {
                let line = build_system_message_line(m);

                if m.message_id <= 4 {
                    tracing::info!(
                        msg_id = m.message_id,
                        text = %line.text,
                        "0x053 SYSTEMMES: server denied zone change",
                    );
                } else if m.message_id == 7 || m.message_id == 35 {
                    tracing::info!(
                        msg_id = m.message_id,
                        seconds = m.para,
                        text = %line.text,
                        "0x053 SYSTEMMES: leavegame countdown tick",
                    );

                    let _ = event_tx.send(AgentEvent::LogoutCountdown {
                        seconds_remaining: m.para.min(u16::MAX as u32) as u16,
                        shutdown: m.message_id == 35,
                    });
                } else {
                    tracing::trace!(
                        msg_id = m.message_id,
                        para = m.para,
                        para2 = m.para2,
                        text = %line.text,
                        "0x053 SYSTEMMES",
                    );
                }
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
        }
        op if op == s2c::GROUP_LIST => {
            if let Ok((attrs, extra)) = decode::PartyAttrs::decode_group_list(sub.data) {
                if attrs.unique_no == self_char_id {
                    note_mog_transition(attrs.moghouse_flg != 0, was_in_mog_house, event_tx);
                }
                let _ = event_tx.send(AgentEvent::PartyMemberUpdated {
                    member: party_member_from_attrs(&attrs, Some(&extra)),
                });
            }
        }
        op if op == s2c::GROUP_ATTR => {
            if let Ok(attrs) = decode::PartyAttrs::decode_group_attr(sub.data) {
                if attrs.unique_no == self_char_id {
                    note_mog_transition(attrs.moghouse_flg != 0, was_in_mog_house, event_tx);
                }
                let _ = event_tx.send(AgentEvent::PartyMemberUpdated {
                    member: party_member_from_attrs(&attrs, None),
                });
            }
        }
        op if op == s2c::ITEM_MAX => {
            if let Ok(m) = decode::ItemMax::decode(sub.data) {
                let summary: Vec<String> = m
                    .capacities
                    .iter()
                    .enumerate()
                    .filter(|(_, &c)| c != 0)
                    .map(|(i, c)| format!("c{}={}", i, c))
                    .collect();
                let _ = event_tx.send(AgentEvent::ChatLine {
                    line: ChatLine {
                        channel: ChatChannel::System,
                        sender: "client".into(),
                        text: format!("📦 Bag capacities: {}", summary.join(", ")),
                        server_ts: 0,
                    },
                });
                let _ = event_tx.send(AgentEvent::InventoryUpdated {
                    container: 0,
                    update: InventoryUpdate::Capacities {
                        capacities: m.capacities.to_vec(),
                    },
                });
            }
        }
        op if op == s2c::ITEM_SAME => {
            if let Ok(s) = decode::ItemSame::decode(sub.data) {
                if matches!(s.state, decode::ItemSameState::AllLoaded) {
                    let _ = event_tx.send(AgentEvent::InventoryReady);
                }
            }
        }
        op if op == s2c::ITEM_NUM => {
            if let Ok(n) = decode::ItemNum::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::ChatLine {
                    line: ChatLine {
                        channel: ChatChannel::Debug,
                        sender: "client".into(),
                        text: format!(
                            "📦 Qty: cat={} slot={} qty→{}{}",
                            n.category,
                            n.index,
                            n.quantity,
                            if n.lock_flg != 0 { " [locked]" } else { "" },
                        ),
                        server_ts: 0,
                    },
                });
                let _ = event_tx.send(AgentEvent::InventoryUpdated {
                    container: n.category,
                    update: InventoryUpdate::QuantityChanged {
                        index: n.index,
                        quantity: n.quantity,
                        locked: n.lock_flg != 0,
                    },
                });
            }
        }
        op if op == s2c::ITEM_LIST => {
            if let Ok(l) = decode::ItemList::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::ChatLine {
                    line: ChatLine {
                        channel: ChatChannel::Debug,
                        sender: "client".into(),
                        text: format!(
                            "📦 Slot: cat={} slot={} item=#{} qty={}",
                            l.category, l.index, l.item_no, l.quantity,
                        ),
                        server_ts: 0,
                    },
                });
                let _ = event_tx.send(AgentEvent::InventoryUpdated {
                    container: l.category,
                    update: InventoryUpdate::SlotChanged {
                        slot: ItemSlot {
                            index: l.index,
                            item_no: l.item_no,
                            quantity: l.quantity,
                            locked: l.lock_flg != 0,

                            price: 0,
                        },
                    },
                });
            }
        }
        op if op == s2c::ITEM_ATTR => {
            if let Ok(a) = decode::ItemAttr::decode(sub.data) {
                let price_tag = if a.price != 0 {
                    format!(" price={}", a.price)
                } else {
                    String::new()
                };
                let _ = event_tx.send(AgentEvent::ChatLine {
                    line: ChatLine {
                        channel: ChatChannel::Debug,
                        sender: "client".into(),
                        text: format!(
                            "📦 Attr: cat={} slot={} item=#{} qty={}{}",
                            a.category, a.index, a.item_no, a.quantity, price_tag,
                        ),
                        server_ts: 0,
                    },
                });
                let _ = event_tx.send(AgentEvent::InventoryUpdated {
                    container: a.category,
                    update: InventoryUpdate::SlotChanged {
                        slot: ItemSlot {
                            index: a.index,
                            item_no: a.item_no,
                            quantity: a.quantity,
                            locked: a.lock_flg != 0,
                            price: a.price,
                        },
                    },
                });
            }
        }
        op if op == s2c::EQUIP_CLEAR => {
            let _ = event_tx.send(AgentEvent::EquipCleared);
        }
        op if op == s2c::EQUIP_LIST => {
            if let Ok(e) = decode::EquipList::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::EquipUpdated {
                    slot: e.equip_slot,
                    container: e.container,
                    container_index: e.container_index,
                });
            }
        }
        op if op == s2c::MAGIC_DATA => {
            if let Ok(m) = decode::MagicData::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::SpellsKnownUpdated { ids: m.known_ids() });
            }
        }
        op if op == s2c::COMMAND_DATA => {
            if let Ok(c) = decode::CommandData::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::CommandDataUpdated {
                    weapon_skills: decode::collect_set_bits(c.weapon_skills),
                    job_abilities: decode::collect_set_bits(c.job_abilities),
                    pet_abilities: decode::collect_set_bits(c.pet_abilities),
                });
            }
        }
        _ => {
            tracing::trace!(
                opcode = format!("0x{:03x}", sub.opcode),
                len = sub.data.len(),
                "unhandled sub-packet"
            );
        }
    }
}

const NAME_MISS_DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

const PENDING_EVENT_END_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

const NAME_MISS_BODY_HEX_CAP: usize = 96;

fn record_name_miss(
    opcode: u16,
    unique_no: u32,
    act_index: u16,
    body: &[u8],
    name_miss_dedup: &mut std::collections::HashMap<
        (u32, crate::state::NameMissKind),
        std::time::Instant,
    >,
    event_tx: &broadcast::Sender<AgentEvent>,
) {
    use crate::state::NameMissKind;
    let send_flag = body.get(6).copied().unwrap_or(0);
    let miss_kind = if send_flag & 0x08 == 0 {
        NameMissKind::NameBitClear
    } else {
        NameMissKind::NameBitSetExtractionFailed
    };
    let now = std::time::Instant::now();
    if let Some(prev) = name_miss_dedup.get(&(unique_no, miss_kind)) {
        if now.duration_since(*prev) < NAME_MISS_DEDUP_WINDOW {
            return;
        }
    }
    name_miss_dedup.insert((unique_no, miss_kind), now);

    let n = body.len().min(NAME_MISS_BODY_HEX_CAP);
    let mut body_hex = String::with_capacity(n * 2);
    for b in &body[..n] {
        use std::fmt::Write;
        let _ = write!(body_hex, "{:02x}", b);
    }
    let at_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let miss = crate::state::NameExtractionMiss {
        opcode,
        unique_no,
        act_index,
        send_flag,
        body_len: body.len(),
        body_hex,
        miss_kind,
        at_unix_ms,
    };

    if miss_kind == NameMissKind::NameBitSetExtractionFailed {
        tracing::debug!(
            opcode = format!("0x{:03x}", opcode),
            unique_no = format!("0x{:08x}", unique_no),
            act_index = format!("0x{:04x}", act_index),
            send_flag = format!("0x{:02x}", send_flag),
            body_len = body.len(),
            "name extraction failed with Name bit SET — investigate offset/validation",
        );
    }
    let _ = event_tx.send(AgentEvent::NameExtractionMiss { miss });
}

fn is_fresh_bundle(last_applied: Option<u16>, incoming: u16) -> bool {
    match last_applied {
        None => true,
        Some(prev) => incoming != prev && incoming.wrapping_sub(prev) < 0x8000,
    }
}

#[allow(clippy::too_many_arguments)]
async fn keepalive_loop(
    map: &mut MapClient,
    mut bundle_seq: u16,
    mut sub_seq: u16,
    mut server_last_seq: u16,
    mut pending_event_end: Vec<(u32, u16, u16)>,
    self_char_id: u32,
    character_name: String,
    mut current_zone_id: u16,
    mut self_act_index: Option<u16>,
    cmd_rx: &mut mpsc::Receiver<AgentCommand>,
    event_tx: broadcast::Sender<AgentEvent>,
    user_driven_events: bool,
    mut name_cache: std::collections::HashMap<u32, String>,
    mut kind_cache: std::collections::HashMap<u32, crate::state::EntityKind>,
    mut claim_cache: std::collections::HashMap<u32, u32>,
    mut name_miss_dedup: std::collections::HashMap<
        (u32, crate::state::NameMissKind),
        std::time::Instant,
    >,
    mut self_pos: Position,

    mut self_pos_seeded: bool,
    mut npc_name_resolver: NpcNameResolver,
) -> Result<MapOutcome> {
    let mut last_recv = std::time::Instant::now();

    let mut net_health = crate::net_health::NetHealth::new();
    let mut last_net_emit = std::time::Instant::now();
    let mut keepalive_send_failing = false;

    let mut enterzone_seen = false;
    let mut zone_transition_sent = false;

    let mut server_seq_applied: Option<u16> = None;

    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    tick.tick().await;
    let mut reconnect_addr: Option<std::net::SocketAddr> = None;

    let mut reconnect_via_zoneline: Option<u32> = None;
    let mut terminal_disconnect = false;

    let mut pending_maprect: Option<(std::time::Instant, u32)> = None;

    let mut pending_event_end_since: Option<std::time::Instant> = None;

    let mut dialog_session =
        crate::event_dialog::DialogSession::new(npc_name_resolver.root.clone());

    let mut is_healing = false;

    let mut last_keepalive_pos: Vec3 = self_pos.pos;

    let mut last_move_emission: Option<std::time::Instant> = None;
    let mut last_emitted_pos: Vec3 = self_pos.pos;
    let mut last_emitted_heading: u8 = self_pos.heading;

    let mut rubber_band_target: Option<Vec3> = None;
    let mut last_rubber_band_step: std::time::Instant = std::time::Instant::now();

    let mut self_in_mog_house = false;
    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                if let Some(c) = cmd.as_ref() {
                    tracing::debug!(variant = ?std::mem::discriminant(c), "cmd_rx recv");
                }
                match cmd {
                    None => break,
                    Some(AgentCommand::Move { x, y, z, heading }) => {
                        self_pos = Position { pos: Vec3 { x, y, z }, heading, ..self_pos };
                        let _ = event_tx.send(AgentEvent::PositionChanged { pos: self_pos });
                    }
                     Some(AgentCommand::StopMove) => {  }
                     Some(AgentCommand::SetFps { max }) => {
                         let _ = event_tx.send(AgentEvent::SetFps { max });
                     }
                     Some(AgentCommand::EndEvent) => {
                        // VM-driven event: advance to the next frame, or send
                        // EVENT_END only once the script ends.
                        if let Some((u, a, n)) = dialog_session.active_end() {
                            match dialog_session.advance(None) {
                                crate::event_dialog::Advance::Frame(dialog) => {
                                    emit_event_speech_to_chat(&event_tx, &dialog);
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                                crate::event_dialog::Advance::Ended { end_para } => {
                                    let payload = build_subpacket_event_end(sub_seq, u, a, n, end_para);
                                    sub_seq = sub_seq.wrapping_add(1);
                                    if let Err(e) = map.send_encrypted(&payload, bundle_seq, server_last_seq).await {
                                        tracing::warn!(error = %e, "EVENT_END (vm) send failed");
                                    }
                                    bundle_seq = bundle_seq.wrapping_add(1);
                                    pending_event_end.retain(|(uid, _, en)| !(*uid == u && *en == n));
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                }
                            }
                        } else if !pending_event_end.is_empty() {
                            let mut payload = Vec::new();
                            for (unique_no, act_index, event_num) in pending_event_end.drain(..) {
                                payload.extend(build_subpacket_event_end(sub_seq, unique_no, act_index, event_num, 0));
                                sub_seq = sub_seq.wrapping_add(1);
                            }
                            if let Err(e) = map.send_encrypted(&payload, bundle_seq, server_last_seq).await {
                                tracing::warn!(error = %e, "EVENT_END send failed");
                            }
                            bundle_seq = bundle_seq.wrapping_add(1);
                            let _ = event_tx.send(AgentEvent::EventEnded);
                        } else {
                            let _ = event_tx.send(AgentEvent::EventEnded);
                        }
                    }
                    Some(AgentCommand::EndEventChoice {
                        event_id,
                        act_index,
                        event_num,
                        choice,
                    }) => {
                        // VM-driven event: feed the selection to the script and
                        // advance; only send EVENT_END once it ends.
                        if let Some((u, a, n)) = dialog_session.active_end() {
                            match dialog_session.advance(Some(choice)) {
                                crate::event_dialog::Advance::Frame(dialog) => {
                                    emit_event_speech_to_chat(&event_tx, &dialog);
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                                crate::event_dialog::Advance::Ended { end_para } => {
                                    let payload = build_subpacket_event_end(sub_seq, u, a, n, end_para);
                                    sub_seq = sub_seq.wrapping_add(1);
                                    if let Err(e) = map.send_encrypted(&payload, bundle_seq, server_last_seq).await {
                                        tracing::warn!(error = %e, "EVENT_END (vm choice) send failed");
                                    }
                                    bundle_seq = bundle_seq.wrapping_add(1);
                                    pending_event_end.retain(|(uid, _, en)| !(*uid == u && *en == n));
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                }
                            }
                        } else {
                            let payload = build_subpacket_event_end(
                                sub_seq, event_id, act_index, event_num, choice,
                            );
                            sub_seq = sub_seq.wrapping_add(1);
                            if let Err(e) = map
                                .send_encrypted(&payload, bundle_seq, server_last_seq)
                                .await
                            {
                                tracing::warn!(error = %e, "EVENT_END (choice) send failed");
                            }
                            bundle_seq = bundle_seq.wrapping_add(1);

                            pending_event_end.retain(|(uid, _, en)| {
                                !(*uid == event_id && *en == event_num)
                            });
                            let _ = event_tx.send(AgentEvent::EventEnded);
                        }
                    }
                    Some(AgentCommand::Disconnect) => {
                        let _ = event_tx.send(AgentEvent::Disconnected { reason: "agent requested disconnect".into() });
                        break;
                    }
                    Some(AgentCommand::ReqLogout { kind }) => {

                        let (mode, kind_wire) = kind.wire_pair();
                        let payload = build_subpacket_reqlogout(sub_seq, mode, kind_wire);
                        tracing::info!(
                            ?kind,
                            mode,
                            kind_wire,
                            sub_seq,
                            bundle_seq,
                            "reqlogout send (0x0E7)"
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "reqlogout send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("reqlogout send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::Snapshot) => {

                        let _ = event_tx.send(AgentEvent::Connected {
                            account_id: 0,
                            char_id: self_char_id,
                            character: character_name.clone(),
                            zone_id: current_zone_id,
                        });
                        let _ = event_tx.send(AgentEvent::StageChanged {
                            stage: Stage::InZone,
                        });
                        let _ = event_tx.send(AgentEvent::PositionChanged { pos: self_pos });
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
                        tracing::info!(
                            kind,
                            len = text.len(),
                            sub_seq,
                            bundle_seq,
                            payload_bytes = payload.len(),
                            "chat send (0x0B5)"
                        );
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
                    Some(AgentCommand::FishingRequest { mode, para, para2 }) => {
                        let payload = build_subpacket_fishing(
                            sub_seq,
                            self_char_id,
                            self_act_index.unwrap_or(0),
                            mode,
                            para,
                            para2,
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "fishing request send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("fishing send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    // The reactor's fishing machine consumes Fish/FishingInput and emits
                    // Action{Fish} + FishingRequest; they never reach the session directly.
                    Some(AgentCommand::Fish) | Some(AgentCommand::FishingInput { .. }) => {}
                    Some(AgentCommand::ReturnToHomePoint) => {

                        let payload = build_subpacket_action(
                            sub_seq,
                            self_char_id,
                            self_act_index.unwrap_or(0),
                            &crate::state::ActionKind::HomepointMenu { status_id: 0 },
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "homepoint_return send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("homepoint_return send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::Follow { .. })
                    | Some(AgentCommand::Engage { .. })
                    | Some(AgentCommand::PathTo { .. })
                    | Some(AgentCommand::Cancel)
                    | Some(AgentCommand::BankWhenFull { .. }) => {

                        let _ = event_tx.send(AgentEvent::Error {
                            message: "reactor goal command reached session loop \
                                      (reactor middleware not wired)"
                                .into(),
                        });
                    }
                    Some(AgentCommand::ShopBuy {
                        shop_no,
                        shop_index,
                        qty,
                    }) => {
                        let payload = build_subpacket_shop_buy(sub_seq, qty, shop_no, shop_index);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "shop_buy send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("buy send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::CheckTarget {
                        target_id,
                        target_index,
                        kind,
                    }) => {
                        let payload = build_subpacket_equip_inspect(
                            sub_seq,
                            target_id,
                            target_index,
                            kind.as_u8(),
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "equip_inspect send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("check send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::Heal { mode }) => {
                        let payload = build_subpacket_camp(sub_seq, mode);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, mode = ?mode, "camp send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("heal send: {e}"),
                            });
                        } else {

                            is_healing = match mode {
                                HealMode::On => true,
                                HealMode::Off => false,
                                HealMode::Toggle => !is_healing,
                            };
                            tracing::info!(?mode, is_healing, "camp send (0x0E8)");
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::Equip {
                        container,
                        container_index,
                        equip_slot,
                    }) => {

                        let payload = build_subpacket_equip_set(
                            sub_seq,
                            container_index,
                            equip_slot,
                            container,
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "equip_set send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("equip_set send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::UseItem {
                        container,
                        slot,
                        item_no: _,
                        target_id,
                        target_index,
                    }) => {

                        let payload = build_subpacket_item_use(
                            sub_seq,
                            target_id,
                            target_index,
                            container,
                            slot,
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, bundle_seq, server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "use_item send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("use_item send: {e}"),
                            });
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::RequestZoneChange { line_id }) => {

                        let Some(act_index) = self_act_index else {
                            let _ = event_tx.send(AgentEvent::Error {
                                message: "RequestZoneChange before self ActIndex \
                                          known (no CHAR_PC for self yet)"
                                    .into(),
                            });
                            continue;
                        };
                        tracing::info!(
                            line_id,
                            pos = format!(
                                "({:.2},{:.2},{:.2})",
                                self_pos.pos.x, self_pos.pos.y, self_pos.pos.z,
                            ),
                            "sending 0x05E MAPRECT",
                        );
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
                        } else {
                            pending_maprect = Some((std::time::Instant::now(), line_id));
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                    Some(AgentCommand::MogHouseExit { kind }) => {

                        let Some(act_index) = self_act_index else {
                            let _ = event_tx.send(AgentEvent::Error {
                                message: "MogHouseExit before self ActIndex known".into(),
                            });
                            continue;
                        };
                        let (exit_bit, exit_mode) = kind.wire_pair();
                        tracing::info!(
                            ?kind,
                            exit_bit,
                            exit_mode,
                            pos = format!(
                                "({:.2},{:.2},{:.2})",
                                self_pos.pos.x, self_pos.pos.y, self_pos.pos.z,
                            ),
                            "sending 0x05E MAPRECT (zmrq mog-house exit)",
                        );
                        let payload = build_subpacket_maprect_mh_exit(
                            sub_seq,
                            exit_bit,
                            exit_mode,
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
                            tracing::warn!(error = %e, "mog-house exit MAPRECT send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("MogHouseExit send: {e}"),
                            });
                        } else {

                            const ZMRQ_LE: u32 =
                                u32::from_le_bytes(*b"zmrq");
                            pending_maprect =
                                Some((std::time::Instant::now(), ZMRQ_LE));
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                }
            }
            _ = tick.tick() => {

                {
                    let (total_sent, total_recv) = map.traffic_totals();
                    net_health.sample_rates(std::time::Instant::now(), total_sent, total_recv);
                    if last_net_emit.elapsed() >= std::time::Duration::from_millis(500) {
                        last_net_emit = std::time::Instant::now();
                        let sample = net_health
                            .snapshot(last_recv.elapsed(), bundle_seq.wrapping_sub(1));
                        let _ = event_tx.send(AgentEvent::NetStats {
                            stats: crate::state::NetStats {
                                send_bps: sample.send_bps,
                                recv_bps: sample.recv_bps,
                                send_health: sample.send_health,
                                recv_health: sample.recv_health,
                            },
                        });
                    }
                }

                if let Some((sent_at, line_id)) = pending_maprect {
                    if sent_at.elapsed() > std::time::Duration::from_secs(3) {
                        tracing::warn!(
                            line_id,
                            elapsed_ms = sent_at.elapsed().as_millis() as u64,
                            "MAPRECT watchdog: server silently dropped zone change",
                        );
                        let _ = event_tx.send(AgentEvent::ChatLine {
                            line: ChatLine {
                                channel: ChatChannel::System,
                                sender: "<client>".into(),
                                text: format!(
                                    "Zone change for line {line_id} silently dropped \
                                     (no server response in 3s). Server-side state \
                                     (likely InEvent flag) is blocking it — try relog."
                                ),
                                server_ts: 0,
                            },
                        });
                        pending_maprect = None;
                    }
                }

                match (pending_event_end.is_empty(), pending_event_end_since.is_some()) {
                    (false, false) => {
                        pending_event_end_since = Some(std::time::Instant::now());
                    }
                    (true, true) => {
                        pending_event_end_since = None;
                    }
                    _ => {}
                }
                let watchdog_fires = pending_event_end_since
                    .map(|t| t.elapsed() > PENDING_EVENT_END_GRACE)
                    .unwrap_or(false);

                let mut payload = Vec::new();

                if enterzone_seen && !zone_transition_sent {
                    payload.extend(build_subpacket_zone_transition(sub_seq));
                    sub_seq = sub_seq.wrapping_add(1);
                    zone_transition_sent = true;
                    tracing::info!(
                        sub_seq,
                        "sent 0x011 ZONE_TRANSITION after 0x008 ENTERZONE (GAMEOK mode)"
                    );
                }
                if (!user_driven_events || watchdog_fires) && !pending_event_end.is_empty() {
                    for (unique_no, act_index, event_num) in pending_event_end.drain(..) {
                        payload.extend(build_subpacket_event_end(
                            sub_seq, unique_no, act_index, event_num, 0,
                        ));
                        sub_seq = sub_seq.wrapping_add(1);
                        let _ = event_tx.send(AgentEvent::EventEnded);
                    }
                    if watchdog_fires {
                        tracing::warn!(
                            grace_secs = PENDING_EVENT_END_GRACE.as_secs(),
                            "auto-flushed pending EVENT_END (watchdog grace expired)"
                        );
                        let _ = event_tx.send(AgentEvent::Error {
                            message: format!(
                                "auto-released pinned event after {}s grace \
                                 (operator didn't /endcutscene or /release)",
                                PENDING_EVENT_END_GRACE.as_secs()
                            ),
                        });
                    }
                    pending_event_end_since = None;
                }

                if let Some(target) = rubber_band_target {
                    let dt = last_rubber_band_step.elapsed().as_secs_f32();
                    last_rubber_band_step = std::time::Instant::now();
                    let max_step = 5.0 * dt;
                    let (next, reached) = lerp_toward(self_pos.pos, target, max_step);
                    self_pos.pos = next;
                    if reached {
                        rubber_band_target = None;
                    }
                } else {
                    last_rubber_band_step = std::time::Instant::now();
                }

                if is_healing && last_keepalive_pos != self_pos.pos {
                    payload.extend(build_subpacket_camp(sub_seq, HealMode::Off));
                    sub_seq = sub_seq.wrapping_add(1);
                    is_healing = false;
                    tracing::info!(
                        from = format!("({:.1},{:.1},{:.1})", last_keepalive_pos.x, last_keepalive_pos.y, last_keepalive_pos.z),
                        to = format!("({:.1},{:.1},{:.1})", self_pos.pos.x, self_pos.pos.y, self_pos.pos.z),
                        "camp auto-cancel (movement detected during heal)"
                    );
                }

                let dx = self_pos.pos.x - last_emitted_pos.x;
                let dy = self_pos.pos.y - last_emitted_pos.y;
                let dz = self_pos.pos.z - last_emitted_pos.z;
                let pos_delta = (dx * dx + dy * dy + dz * dz).sqrt();
                let heading_changed = self_pos.heading != last_emitted_heading;
                let include_pos = self_pos_seeded
                    && match last_move_emission {
                        None => true,
                        Some(t) => should_emit_pos(t.elapsed(), pos_delta, heading_changed),
                    };
                if include_pos {
                    payload.extend(build_subpacket_pos(
                        sub_seq,
                        self_pos.pos.x,
                        self_pos.pos.y,
                        self_pos.pos.z,
                        self_pos.heading,
                    ));
                    sub_seq = sub_seq.wrapping_add(1);
                    last_keepalive_pos = self_pos.pos;
                    last_emitted_pos = self_pos.pos;
                    last_emitted_heading = self_pos.heading;
                    last_move_emission = Some(std::time::Instant::now());
                }

                if !payload.is_empty() {
                    match map.send_encrypted(&payload, bundle_seq, server_last_seq).await {
                        Ok(()) => {
                            if keepalive_send_failing {
                                keepalive_send_failing = false;
                                tracing::info!("keepalive send recovered");
                            }
                            bundle_seq = bundle_seq.wrapping_add(1);
                        }
                        // A failed keepalive send (link down) must NOT tear the session
                        // down: retail holds the connection and decays the network-health
                        // % while no packets flow, disconnecting only at the silence
                        // timeout below. Hold and let last_recv age drive the decay.
                        Err(e) => {
                            if !keepalive_send_failing {
                                keepalive_send_failing = true;
                                tracing::warn!(error = %e, "keepalive send failing (link down?); holding until silence timeout");
                            }
                        }
                    }
                }
            }
            res = tokio::time::timeout(std::time::Duration::from_millis(50), map.recv_decrypted()) => {
                if let Ok(Ok(buf)) = res {
                    last_recv = std::time::Instant::now();
                    let header = framing::Header::read(&buf[..framing::FFXI_HEADER_SIZE]);

                    server_last_seq = header.id_and_size;

                    if !is_fresh_bundle(server_seq_applied, server_last_seq) {
                        continue;
                    }
                    server_seq_applied = Some(server_last_seq);
                    // datagram byte[2..4] (sync_in) on an inbound packet is the server's
                    // ack of our client seq (LSB MapSession::client_packet_id, written by
                    // preparePacket at vendor/server/src/map/map_networking.cpp:654).
                    net_health.on_recv(server_last_seq, header.sync_in);
                    for sub in framing::walk_sub_packets(&buf[framing::FFXI_HEADER_SIZE..]).flatten() {
                        if sub.opcode == ffxi_proto::map::s2c::LOGOUT {
                            if let Ok(logout) = decode::ServerLogout::decode(sub.data) {
                                if logout.is_zone_change() {
                                    let new_addr = parse_logout_addr(&logout, map.server_addr());
                                    let _ = event_tx.send(AgentEvent::ZoneChanged {
                                        from: None,
                                        to: 0,
                                    });
                                    reconnect_addr = Some(new_addr);

                                    reconnect_via_zoneline =
                                        pending_maprect.map(|(_, line_id)| line_id);
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

                            if !zone_transition_sent
                                && sub.opcode == ffxi_proto::map::s2c::ENTERZONE
                            {
                                enterzone_seen = true;
                            }

                            // Drive event triggers (0x32/0x33/0x34) through the
                            // event VM for faithful dialog; fall back to the raw
                            // decoder in handle_sub_packet when no DAT can run it.
                            if matches!(
                                sub.opcode,
                                ffxi_proto::map::s2c::EVENT
                                    | ffxi_proto::map::s2c::EVENTSTR
                                    | ffxi_proto::map::s2c::EVENTNUM
                            ) {
                                if let Some((unique_no, act_index, event_id)) =
                                    event_trigger_ids(&sub)
                                {
                                    let name = name_cache.get(&unique_no).cloned().or_else(|| {
                                        npc_name_resolver
                                            .lookup(unique_no)
                                            .map(|s| s.replace('_', " "))
                                    });
                                    if let Some(dialog) = dialog_session.begin(
                                        current_zone_id,
                                        unique_no,
                                        act_index,
                                        event_id,
                                        name,
                                    ) {
                                        let _ = event_tx.send(AgentEvent::EventStart {
                                            event_id: dialog.event_id,
                                        });
                                        emit_event_speech_to_chat(&event_tx, &dialog);
                                        let _ = event_tx
                                            .send(AgentEvent::EventDialog { dialog });
                                        pending_event_end.push((
                                            unique_no, act_index, event_id,
                                        ));
                                        continue;
                                    }
                                }
                            }

                            let prev_self_pos = self_pos.pos;
                            handle_sub_packet(
                                &sub,
                                &event_tx,
                                &mut pending_event_end,
                                self_char_id,
                                &character_name,
                                &mut self_act_index,
                                &mut name_cache,
                                &mut kind_cache,
                                &mut claim_cache,
                                &mut name_miss_dedup,
                                &mut current_zone_id,
                                &mut self_pos,
                                &mut self_pos_seeded,
                                &mut npc_name_resolver,
                                &mut self_in_mog_house,

                                None,
                            );

                            if sub.opcode == ffxi_proto::map::s2c::CHAR_PC {
                                if let Ok(head) = decode::PosHead::decode(sub.data) {
                                    if head.unique_no == self_char_id {
                                        let server_pos = self_pos.pos;
                                        match reconcile_self_pos(prev_self_pos, server_pos) {
                                            SelfPosReconcile::KeepLocal => {

                                                self_pos.pos = prev_self_pos;
                                                rubber_band_target = None;
                                            }
                                            SelfPosReconcile::Rubberband { target } => {

                                                self_pos.pos = prev_self_pos;
                                                rubber_band_target = Some(target);
                                                last_rubber_band_step =
                                                    std::time::Instant::now();
                                                tracing::debug!(
                                                    from = format!(
                                                        "({:.1},{:.1},{:.1})",
                                                        prev_self_pos.x,
                                                        prev_self_pos.y,
                                                        prev_self_pos.z,
                                                    ),
                                                    to = format!(
                                                        "({:.1},{:.1},{:.1})",
                                                        target.x, target.y, target.z,
                                                    ),
                                                    "rubber-band self pos toward server",
                                                );
                                            }
                                            SelfPosReconcile::Snap => {

                                                rubber_band_target = None;
                                                tracing::info!(
                                                    to = format!(
                                                        "({:.1},{:.1},{:.1})",
                                                        server_pos.x,
                                                        server_pos.y,
                                                        server_pos.z,
                                                    ),
                                                    "snap self pos to server (>10 yalm delta)",
                                                );
                                            }
                                        }
                                    }
                                }
                            }

                            if sub.opcode == ffxi_proto::map::s2c::CHAR_PC {
                                if let Ok(head) = decode::PosHead::decode(sub.data) {
                                    let send_flag = sub.data.get(6).copied().unwrap_or(0);
                                    if head.unique_no == self_char_id && (send_flag & 0x04) != 0 {
                                        let server_healing =
                                            head.server_status == decode::animation::HEALING;
                                        if is_healing != server_healing {
                                            tracing::info!(
                                                was = is_healing,
                                                now = server_healing,
                                                animation = head.server_status,
                                                "heal state synced from CHAR_PC"
                                            );
                                            is_healing = server_healing;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if reconnect_addr.is_some() || terminal_disconnect {
            break;
        }

        if last_recv.elapsed() > crate::net_health::MAP_SILENCE_TIMEOUT {
            let _ = event_tx.send(AgentEvent::Disconnected {
                reason: "no server packets for 60s".into(),
            });
            break;
        }
    }

    if let Some(addr) = reconnect_addr {
        Ok(MapOutcome::Reconnect {
            new_addr: addr,
            via_zoneline: reconnect_via_zoneline,
        })
    } else {
        Ok(MapOutcome::Disconnected)
    }
}

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

pub async fn run_event_folder(
    mut event_rx: broadcast::Receiver<AgentEvent>,
    state_tx: tokio::sync::watch::Sender<crate::state::SessionState>,
) {
    use tokio::sync::broadcast::error::RecvError;
    let mut total_dropped: u64 = 0;
    loop {
        match event_rx.recv().await {
            Ok(event) => state_tx.send_modify(|s| s.apply_event(&event)),
            Err(RecvError::Lagged(n)) => {
                total_dropped += n;
                tracing::warn!(
                    dropped = n,
                    total_dropped,
                    "run_event_folder lagged — dropped events (folded state now \
                     stale; a lost zone-in self-seed shows up as /pos 0,0,0)"
                );
            }
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

fn build_subpacket_gameok(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x00C, 3, sync));
    buf
}

fn build_subpacket_zone_transition(sync: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x011, 2, sync));

    buf[4] = 2;
    buf
}

fn build_system_message_line(m: decode::SystemMessage) -> ChatLine {
    let text = match ffxi_proto::msg_system::lookup(m.message_id) {
        Some(raw) => substitute_system_placeholders(raw, m.para, m.para2),
        None => format!("[system] msg #{} para={},{}", m.message_id, m.para, m.para2),
    };
    ChatLine {
        channel: ChatChannel::System,
        sender: "<server>".into(),
        text,
        server_ts: 0,
    }
}

fn substitute_system_placeholders(raw: &str, para: u32, para2: u32) -> String {
    let p = para.to_string();
    let mut s = raw.to_string();
    for tag in [
        "<seconds>",
        "<number>",
        "<param>",
        "<value>",
        "<amount>",
        "<n>",
        "<gil>",
    ] {
        s = s.replace(tag, &p);
    }
    if s.contains("<number2>") {
        s = s.replace("<number2>", &para2.to_string());
    }
    s
}

fn emit_battle_message_audio_event(
    data: &[u8],
    is_029: bool,
    event_tx: &tokio::sync::broadcast::Sender<AgentEvent>,
) {
    if data.len() < 24 {
        return;
    }
    let cas_id = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let (data1, data2) = if is_029 {
        (
            u32::from_le_bytes(data[8..12].try_into().unwrap()),
            u32::from_le_bytes(data[12..16].try_into().unwrap()),
        )
    } else {
        (
            u32::from_le_bytes(data[12..16].try_into().unwrap()),
            u32::from_le_bytes(data[16..20].try_into().unwrap()),
        )
    };
    let message_num = u16::from_le_bytes(data[20..22].try_into().unwrap());
    match message_num {
        9 => {
            let _ = event_tx.send(AgentEvent::LevelUp { player_id: cas_id });
        }
        53 => {
            let _ = event_tx.send(AgentEvent::SkillLevelUp {
                skill_id: data1 as u16,
                level: data2,
            });
        }
        _ => {}
    }
}

fn decode_battle_message(
    data: &[u8],
    name_cache: &std::collections::HashMap<u32, String>,
    kind_cache: &std::collections::HashMap<u32, crate::state::EntityKind>,
    is_029: bool,
) -> Option<ChatLine> {
    if data.len() < 24 {
        return None;
    }
    let cas_id = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let tar_id = u32::from_le_bytes(data[4..8].try_into().unwrap());
    let (data1, data2) = if is_029 {
        (
            u32::from_le_bytes(data[8..12].try_into().unwrap()),
            u32::from_le_bytes(data[12..16].try_into().unwrap()),
        )
    } else {
        (
            u32::from_le_bytes(data[12..16].try_into().unwrap()),
            u32::from_le_bytes(data[16..20].try_into().unwrap()),
        )
    };
    let message_num = u16::from_le_bytes(data[20..22].try_into().unwrap());

    let cas_name = name_for_id(cas_id, name_cache);
    let tar_name = name_for_id(tar_id, name_cache);
    if let Some(text) = synth_check_line(message_num, data1, data2, &cas_name, &tar_name) {
        return Some(ChatLine {
            channel: ChatChannel::Battle,
            sender: cas_name,
            text,
            server_ts: 0,
        });
    }
    let raw = template_for_id(message_num)?;
    let text = substitute_battle_placeholders(
        raw,
        &cas_name,
        &tar_name,
        is_pc(cas_id, kind_cache),
        is_pc(tar_id, kind_cache),
        data1,
        data2,
        message_num,
        None,
    );
    Some(ChatLine {
        channel: ChatChannel::Battle,

        sender: if subject_is_tar(message_num) {
            tar_name
        } else {
            cas_name
        },
        text,
        server_ts: 0,
    })
}

struct BattleBitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BattleBitReader<'a> {
    fn new(data: &'a [u8], start_bit: usize) -> Self {
        Self {
            data,
            pos: start_bit,
        }
    }

    fn read(&mut self, bits: u32) -> Option<u64> {
        debug_assert!(bits <= 32);
        let byte_offset = self.pos / 8;
        let bit_in_byte = self.pos % 8;
        let total_bits = bits as usize + bit_in_byte;
        let value: u64 = if total_bits <= 8 {
            *self.data.get(byte_offset)? as u64
        } else if total_bits <= 16 {
            if byte_offset + 2 > self.data.len() {
                return None;
            }
            u16::from_le_bytes(self.data[byte_offset..byte_offset + 2].try_into().ok()?) as u64
        } else if total_bits <= 32 {
            if byte_offset + 4 > self.data.len() {
                return None;
            }
            u32::from_le_bytes(self.data[byte_offset..byte_offset + 4].try_into().ok()?) as u64
        } else {
            if byte_offset + 8 > self.data.len() {
                return None;
            }
            u64::from_le_bytes(self.data[byte_offset..byte_offset + 8].try_into().ok()?)
        };
        let mask = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        self.pos += bits as usize;
        Some((value >> bit_in_byte) & mask)
    }
}

pub fn decode_battle2_header(data: &[u8]) -> Option<(u32, u32, u8)> {
    let mut br = BattleBitReader::new(data, 8);
    let actor_id = br.read(32)? as u32;
    let _trg_sum = br.read(6)?;
    let _res_sum = br.read(4)?;
    let action_kind = br.read(4)? as u8;
    let action_id = br.read(32)? as u32;
    Some((actor_id, action_id, action_kind))
}

fn decode_battle2_action(
    data: &[u8],
    name_cache: &std::collections::HashMap<u32, String>,
    kind_cache: &std::collections::HashMap<u32, crate::state::EntityKind>,
) -> Vec<ChatLine> {
    let mut out: Vec<ChatLine> = Vec::new();

    let mut br = BattleBitReader::new(data, 8);

    let actor_id = match br.read(32) {
        Some(v) => v as u32,
        None => return out,
    };
    let trg_sum = br.read(6).unwrap_or(0) as usize;
    let _res_sum = br.read(4);

    let cmd_no = br.read(4).unwrap_or(0) as u8;
    let cmd_arg = match br.read(32) {
        Some(v) => v as u32,
        None => return out,
    };
    let _info = br.read(32);

    let cas_name = name_for_id(actor_id, name_cache);
    let cas_is_pc = is_pc(actor_id, kind_cache);

    for _t in 0..trg_sum.min(15) {
        let Some(target_id) = br.read(32) else {
            return out;
        };
        let result_sum = br.read(4).unwrap_or(0) as usize;
        let tar_name = name_for_id(target_id as u32, name_cache);
        let tar_is_pc = is_pc(target_id as u32, kind_cache);

        for _r in 0..result_sum.min(8) {
            let _miss = br.read(3);
            let _kind = br.read(2);
            let _sub_kind = br.read(12);
            let _info = br.read(5);
            let _scale = br.read(5);
            let value = br.read(17).unwrap_or(0) as u32;
            let message_num = br.read(10).unwrap_or(0) as u16;
            let _modifier = br.read(31);

            let has_proc = br.read(1).unwrap_or(0) != 0;
            let mut proc_message: u16 = 0;
            let mut proc_value: u32 = 0;
            if has_proc {
                let _proc_kind = br.read(6);
                let _proc_info = br.read(4);
                proc_value = br.read(17).unwrap_or(0) as u32;
                proc_message = br.read(10).unwrap_or(0) as u16;
            }

            let has_react = br.read(1).unwrap_or(0) != 0;
            let mut react_message: u16 = 0;
            let mut react_value: u32 = 0;
            if has_react {
                let _react_kind = br.read(6);
                let _react_info = br.read(4);
                react_value = br.read(14).unwrap_or(0) as u32;
                react_message = br.read(10).unwrap_or(0) as u16;
            }

            if message_num != 0 {
                if let Some(line) = build_battle2_line(
                    message_num,
                    &cas_name,
                    &tar_name,
                    cas_is_pc,
                    tar_is_pc,
                    value,
                    cmd_arg,
                    cmd_no,
                ) {
                    out.push(line);
                }
            }

            if has_proc && proc_message != 0 {
                if let Some(line) = build_battle2_line(
                    proc_message,
                    &cas_name,
                    &tar_name,
                    cas_is_pc,
                    tar_is_pc,
                    proc_value,
                    cmd_arg,
                    cmd_no,
                ) {
                    out.push(line);
                }
            }

            if has_react && react_message != 0 {
                if let Some(line) = build_battle2_line(
                    react_message,
                    &cas_name,
                    &tar_name,
                    cas_is_pc,
                    tar_is_pc,
                    react_value,
                    cmd_arg,
                    cmd_no,
                ) {
                    out.push(line);
                }
            }
        }
    }

    out
}

fn is_start_category(cmd_no: u8) -> bool {
    matches!(cmd_no, 7 | 8 | 9 | 10 | 12)
}

fn build_battle2_line(
    message_num: u16,
    cas_name: &str,
    tar_name: &str,
    cas_is_pc: bool,
    tar_is_pc: bool,
    amount: u32,
    action_id: u32,
    category: u8,
) -> Option<ChatLine> {
    let raw = template_for_id(message_num)?;

    let resource_id = if is_start_category(category) {
        amount
    } else {
        action_id
    };
    let text = substitute_battle_placeholders(
        raw,
        cas_name,
        tar_name,
        cas_is_pc,
        tar_is_pc,
        amount,
        0,
        message_num,
        Some(resource_id),
    );
    Some(ChatLine {
        channel: ChatChannel::Battle,
        sender: if subject_is_tar(message_num) {
            tar_name.to_string()
        } else {
            cas_name.to_string()
        },
        text,
        server_ts: 0,
    })
}

fn template_for_id(message_num: u16) -> Option<&'static str> {
    for &(id, template) in TEMPLATE_OVERRIDES {
        if id == message_num {
            return Some(template);
        }
    }
    ffxi_proto::msg_basic::lookup(message_num)
}

fn synth_check_line(
    message_num: u16,
    data1: u32,
    data2: u32,
    cas_name: &str,
    tar_name: &str,
) -> Option<String> {
    match message_num {
        170..=178 => Some(render_check_mob(message_num, data1, data2, tar_name)),

        712 => Some(format!("Main weapon — Accuracy: {data1}, Attack: {data2}.")),
        713 => {
            if data1 == 0 && data2 == 0 {
                Some("Auxiliary weapon: none equipped.".to_string())
            } else {
                Some(format!(
                    "Auxiliary weapon — Accuracy: {data1}, Attack: {data2}."
                ))
            }
        }
        714 => {
            if data1 == 0 && data2 == 0 {
                Some("Ranged weapon: none equipped.".to_string())
            } else {
                Some(format!(
                    "Ranged weapon — Accuracy: {data1}, Attack: {data2}."
                ))
            }
        }
        715 => Some(format!("Evasion: {data1}, Defense: {data2}.")),
        731 => Some(format!("Checking {tar_name}'s item level…")),
        733 => Some(format!("Checking {cas_name}'s parameters on {tar_name}.")),

        _ => None,
    }
}

fn render_check_mob(message_num: u16, data1: u32, data2: u32, tar_name: &str) -> String {
    let total: i32 = message_num as i32 - 174;

    let eva_off = if total <= -2 {
        -3
    } else if total >= 2 {
        3
    } else {
        0
    };
    let def_off = total - eva_off;

    let difficulty = match data2.saturating_sub(64) {
        0 => "Too Weak",
        1 => "Incredibly Easy Prey",
        2 => "Easy Prey",
        3 => "Decent Challenge",
        4 => "Even Match",
        5 => "Tough",
        6 => "Very Tough",
        7 => "Incredibly Tough",

        _ => "Unknown",
    };

    let mut line = format!("{tar_name} (Lv. {data1}) — {difficulty}.");
    let def_str = match def_off {
        -1 => Some("high defense"),
        1 => Some("low defense"),
        _ => None,
    };
    let eva_str = match eva_off {
        -3 => Some("high evasion"),
        3 => Some("low evasion"),
        _ => None,
    };
    match (def_str, eva_str) {
        (Some(d), Some(e)) => line.push_str(&format!(" It has {d} and {e}.")),
        (Some(d), None) => line.push_str(&format!(" It has {d}.")),
        (None, Some(e)) => line.push_str(&format!(" It has {e}.")),
        (None, None) => {}
    }
    line
}

const TEMPLATE_OVERRIDES: &[(u16, &str)] = &[(565, "<target> obtains <amount> gil.")];

fn subject_is_tar(message_num: u16) -> bool {
    matches!(message_num, 97)
}

fn name_for_id(id: u32, name_cache: &std::collections::HashMap<u32, String>) -> String {
    if id == 0 {
        return "<no one>".to_string();
    }
    name_cache
        .get(&id)
        .cloned()
        .unwrap_or_else(|| format!("#{:08X}", id))
}

fn is_pc(id: u32, kind_cache: &std::collections::HashMap<u32, crate::state::EntityKind>) -> bool {
    matches!(kind_cache.get(&id), Some(crate::state::EntityKind::Pc))
}

fn replace_named_token(s: &str, tok: &str, name: &str, entity_is_pc: bool) -> String {
    if entity_is_pc {
        s.replace(&format!("The {tok}"), name)
            .replace(&format!("the {tok}"), name)
            .replace(tok, name)
    } else {
        s.replace(tok, name)
    }
}

fn substitute_battle_placeholders(
    raw: &str,
    cas_name: &str,
    tar_name: &str,
    cas_is_pc: bool,
    tar_is_pc: bool,
    data1: u32,
    data2: u32,
    message_num: u16,
    action_id: Option<u32>,
) -> String {
    let mut s = raw.to_string();

    for tag in ["<user>", "<attacker>", "<caster>", "<entity>"] {
        s = replace_named_token(&s, tag, cas_name, cas_is_pc);
    }

    let (player_name, target_name, player_is_pc, target_is_pc) = if subject_is_tar(message_num) {
        (tar_name, cas_name, tar_is_pc, cas_is_pc)
    } else {
        (cas_name, tar_name, cas_is_pc, tar_is_pc)
    };
    s = replace_named_token(&s, "<player>", player_name, player_is_pc);
    s = replace_named_token(&s, "<target>", target_name, target_is_pc);

    s = replace_named_token(&s, "<mob>", target_name, target_is_pc);
    let amount = data1.to_string();
    for tag in ["<amount>", "<number>"] {
        s = s.replace(tag, &amount);
    }
    if s.contains("<number2>") {
        s = s.replace("<number2>", &data2.to_string());
    }
    if s.contains("<skill>") {
        let skill = ffxi_proto::skill_names::lookup(data1 as u8)
            .map(str::to_string)
            .unwrap_or_else(|| format!("skill #{}", data1));
        s = s.replace("<skill>", &skill);
    }

    let resolved_action_id = action_id.unwrap_or(data1);
    if s.contains("<spell>") {
        let name = ffxi_proto::spell_names::lookup(resolved_action_id as u16)
            .map(str::to_string)
            .unwrap_or_else(|| format!("spell #{resolved_action_id}"));
        s = s.replace("<spell>", &name);
    }
    if s.contains("<ability>") {
        let name = ffxi_proto::ability_names::lookup(resolved_action_id as u16)
            .map(str::to_string)
            .unwrap_or_else(|| format!("ability #{resolved_action_id}"));
        s = s.replace("<ability>", &name);
    }
    if s.contains("<item>") {
        let name = ffxi_proto::item_names::lookup(resolved_action_id as u16)
            .map(str::to_string)
            .unwrap_or_else(|| format!("item #{resolved_action_id}"));
        s = s.replace("<item>", &name);
    }
    if s.contains("<job>") {
        let name = ffxi_proto::job_names::lookup(resolved_action_id as u16)
            .map(str::to_string)
            .unwrap_or_else(|| format!("job #{resolved_action_id}"));
        s = s.replace("<job>", &name);
    }
    if s.contains("<status>") {
        let name = ffxi_proto::status_names::lookup(data1 as u16)
            .map(str::to_string)
            .unwrap_or_else(|| format!("status #{data1}"));
        s = s.replace("<status>", &name);
    }

    if message_num == 253 {
        s = replace_marker_nth(&s, '#', 0, &data2.to_string());
        s = replace_marker_nth(&s, '#', 0, &data1.to_string());
    } else {
        s = replace_marker_all(&s, '#', &data1.to_string());
    }

    let x_value = if matches!(message_num, 38 | 310) {
        format_decimal_tenths(data2)
    } else {
        data2.to_string()
    };
    s = replace_marker_all(&s, 'X', &x_value);
    s
}

fn format_decimal_tenths(tenths: u32) -> String {
    format!("{}.{}", tenths / 10, tenths % 10)
}

fn replace_marker_all(src: &str, marker: char, value: &str) -> String {
    let mut out = String::with_capacity(src.len() + value.len());
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == marker && is_token_boundary(&chars, i) {
            out.push_str(value);
        } else {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

fn replace_marker_nth(src: &str, marker: char, n: usize, value: &str) -> String {
    let mut out = String::with_capacity(src.len() + value.len());
    let chars: Vec<char> = src.chars().collect();
    let mut seen = 0usize;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == marker && is_token_boundary(&chars, i) {
            if seen == n {
                out.push_str(value);
                out.extend(chars[i + 1..].iter());
                return out;
            }
            seen += 1;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn is_token_boundary(chars: &[char], i: usize) -> bool {
    let left_ok = i == 0 || !chars[i - 1].is_alphanumeric() && chars[i - 1] != '_';
    let right_ok = i + 1 == chars.len() || !chars[i + 1].is_alphanumeric() && chars[i + 1] != '_';
    left_ok && right_ok
}

fn decode_event_0x032(data: &[u8]) -> Option<crate::state::DialogState> {
    if data.len() < 16 {
        return None;
    }
    let unique_no = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let act_index = u16::from_le_bytes(data[4..6].try_into().unwrap());
    let event_num = u16::from_le_bytes(data[6..8].try_into().unwrap());
    let event_para = u16::from_le_bytes(data[8..10].try_into().unwrap());
    let mode = u16::from_le_bytes(data[10..12].try_into().unwrap());
    let event_num2 = u16::from_le_bytes(data[12..14].try_into().unwrap());
    let event_para2 = u16::from_le_bytes(data[14..16].try_into().unwrap());
    Some(crate::state::DialogState {
        event_id: ((unique_no as u64) << 16 | event_num as u64) as u32,
        npc_id: unique_no,
        npc_name: None,
        act_index,
        event_num,
        event_para,
        mode,
        event_num2,
        event_para2,
        strings: Vec::new(),
        nums: Vec::new(),
        prompt: None,
        choices: Vec::new(),
    })
}

fn decode_event_0x033(data: &[u8]) -> Option<crate::state::DialogState> {
    if data.len() < 108 {
        return None;
    }
    let unique_no = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let act_index = u16::from_le_bytes(data[4..6].try_into().unwrap());
    let event_num = u16::from_le_bytes(data[6..8].try_into().unwrap());
    let event_para = u16::from_le_bytes(data[8..10].try_into().unwrap());
    let mode = u16::from_le_bytes(data[10..12].try_into().unwrap());

    let mut strings: Vec<String> = (0..4)
        .map(|i| {
            let off = 12 + i * 16;
            trim_nul_string(&data[off..off + 16])
        })
        .collect();
    while strings.last().map(String::is_empty).unwrap_or(false) {
        strings.pop();
    }

    let nums: Vec<i32> = (0..8)
        .map(|i| {
            let off = 76 + i * 4;
            i32::from_le_bytes(data[off..off + 4].try_into().unwrap())
        })
        .collect();

    Some(crate::state::DialogState {
        event_id: ((unique_no as u64) << 16 | event_num as u64) as u32,
        npc_id: unique_no,
        npc_name: None,
        act_index,
        event_num,
        event_para,
        mode,

        event_num2: 0,
        event_para2: 0,
        strings,
        nums,
        prompt: None,
        choices: Vec::new(),
    })
}

fn decode_event_0x034(data: &[u8]) -> Option<crate::state::DialogState> {
    if data.len() < 48 {
        return None;
    }
    let unique_no = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let nums: Vec<i32> = (0..8)
        .map(|i| {
            let off = 4 + i * 4;
            i32::from_le_bytes(data[off..off + 4].try_into().unwrap())
        })
        .collect();
    let act_index = u16::from_le_bytes(data[36..38].try_into().unwrap());
    let event_num = u16::from_le_bytes(data[38..40].try_into().unwrap());
    let event_para = u16::from_le_bytes(data[40..42].try_into().unwrap());
    let mode = u16::from_le_bytes(data[42..44].try_into().unwrap());
    let event_num2 = u16::from_le_bytes(data[44..46].try_into().unwrap());
    let event_para2 = u16::from_le_bytes(data[46..48].try_into().unwrap());
    Some(crate::state::DialogState {
        event_id: ((unique_no as u64) << 16 | event_num as u64) as u32,
        npc_id: unique_no,
        npc_name: None,
        act_index,
        event_num,
        event_para,
        mode,
        event_num2,
        event_para2,
        strings: Vec::new(),
        nums,
        prompt: None,
        choices: Vec::new(),
    })
}

/// `(unique_no, act_index, event_id)` from an event-trigger packet (0x32/0x33/
/// 0x34), reusing the raw decoders. The event id the client runs is `EventPara`
/// (`dialog.event_para`), NOT `EventNum` — LSB sets `EventNum = PChar->getZone()`
/// and `EventPara = eventInfo->eventId` (vendor/server/src/map/packets/s2c/
/// 0x032_event.cpp, 0x034_eventnum.cpp). The same `EventPara` is what the server
/// validates on the 0x05B EVENT_END (`isInEvent(EventPara)`).
fn event_trigger_ids(sub: &framing::SubPacket<'_>) -> Option<(u32, u16, u16)> {
    use ffxi_proto::map::s2c;
    let dialog = match sub.opcode {
        op if op == s2c::EVENT => decode_event_0x032(sub.data)?,
        op if op == s2c::EVENTSTR => decode_event_0x033(sub.data)?,
        op if op == s2c::EVENTNUM => decode_event_0x034(sub.data)?,
        _ => return None,
    };
    Some((dialog.npc_id, dialog.act_index, dialog.event_para))
}

fn decode_shop_list(data: &[u8]) -> Option<ShopState> {
    const HEADER_LEN: usize = 4;
    const ROW_LEN: usize = 12;
    if data.len() < HEADER_LEN {
        return None;
    }
    let offset_index = u16::from_le_bytes(data[0..2].try_into().unwrap());
    let row_bytes = &data[HEADER_LEN..];
    let row_count = row_bytes.len() / ROW_LEN;
    let mut items = Vec::with_capacity(row_count);
    for i in 0..row_count {
        let off = i * ROW_LEN;
        let row = &row_bytes[off..off + ROW_LEN];
        let item_no = u16::from_le_bytes(row[4..6].try_into().unwrap());

        if item_no == 0 {
            continue;
        }
        items.push(ShopItem {
            price: u32::from_le_bytes(row[0..4].try_into().unwrap()),
            item_no,
            shop_index: row[6],

            skill: u16::from_le_bytes(row[8..10].try_into().unwrap()),
            guild_info: u16::from_le_bytes(row[10..12].try_into().unwrap()),
        });
    }
    Some(ShopState {
        offset_index,
        items,

        opened: false,
    })
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// vendor/server/src/common/earth_time.h — Unix time of the Vana'diel epoch.
const VANA_EPOCH_UNIX: u64 = 1_009_810_800;

// vendor/server/src/map/packets/s2c/0x063_miscdata_status_icons.cpp:
// timestamp = (remaining_seconds + vanadiel_timestamp()) * 60, u32-wrapping;
// 0x7FFFFFFF marks a no-timer (permanent) effect. Recover absolute Unix expiry,
// returning 0 for permanent / already-expired / implausible values.
fn status_icon_expiry_unix(timestamp: u32, now_unix: u64) -> u32 {
    const NO_TIMER: u32 = 0x7FFF_FFFF;
    if timestamp == 0 || timestamp == NO_TIMER {
        return 0;
    }
    let vana_now = now_unix.saturating_sub(VANA_EPOCH_UNIX) as u32;
    let remaining = timestamp.wrapping_sub(vana_now.wrapping_mul(60)) / 60;
    if remaining == 0 || remaining > 100 * 3600 {
        return 0;
    }
    (now_unix + remaining as u64) as u32
}

fn decode_miscdata_status_icons(data: &[u8]) -> Option<(Vec<u16>, Vec<u32>)> {
    const TYPE_OFFSET: usize = 0;
    const ICONS_OFFSET: usize = 4;
    const ICONS_COUNT: usize = 32;
    const ICONS_BYTES: usize = ICONS_COUNT * 2;
    const TS_OFFSET: usize = ICONS_OFFSET + ICONS_BYTES;
    const PLACEHOLDER: u16 = 0x00FF;

    if data.len() < ICONS_OFFSET + ICONS_BYTES {
        return None;
    }
    let kind = u16::from_le_bytes(data[TYPE_OFFSET..TYPE_OFFSET + 2].try_into().unwrap());
    if kind != 0x0009 {
        return None;
    }
    let now_unix = now_unix_secs();
    let mut icons = Vec::new();
    let mut expiries = Vec::new();
    for i in 0..ICONS_COUNT {
        let off = ICONS_OFFSET + i * 2;
        let icon = u16::from_le_bytes(data[off..off + 2].try_into().unwrap());
        if icon == PLACEHOLDER || icon == 0 {
            continue;
        }
        let ts_off = TS_OFFSET + i * 4;
        let timestamp = if data.len() >= ts_off + 4 {
            u32::from_le_bytes(data[ts_off..ts_off + 4].try_into().unwrap())
        } else {
            0
        };
        icons.push(icon);
        expiries.push(status_icon_expiry_unix(timestamp, now_unix));
    }
    Some((icons, expiries))
}

// vendor/server/src/map/packets/s2c/0x119_abil_recast.h — recasttimer_t[31]:
// u16 Timer (remaining seconds), u8 Calc1, u8 TimerId (recast group id), u16 Calc2,
// u16 padding. Returns (recast_id, absolute Unix expiry) for entries still running.
fn decode_abil_recast(data: &[u8]) -> Vec<(u16, u32)> {
    const ENTRY_SIZE: usize = 8;
    const ENTRY_COUNT: usize = 31;
    let now_unix = now_unix_secs();
    let mut out = Vec::new();
    for i in 0..ENTRY_COUNT {
        let off = i * ENTRY_SIZE;
        if data.len() < off + ENTRY_SIZE {
            break;
        }
        let timer = u16::from_le_bytes(data[off..off + 2].try_into().unwrap());
        let timer_id = data[off + 3] as u16;
        if timer == 0 {
            continue;
        }
        out.push((timer_id, (now_unix + timer as u64) as u32));
    }
    out
}

pub fn build_subpacket_shop_buy(sync: u16, qty: u32, shop_no: u16, shop_index: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x083, 4, sync));
    buf[4..8].copy_from_slice(&qty.to_le_bytes());
    buf[8..10].copy_from_slice(&shop_no.to_le_bytes());
    buf[10..12].copy_from_slice(&(shop_index as u16).to_le_bytes());
    buf[12] = 0;
    buf
}

/// Copy an NPC's event *speech* (a VM message frame — `prompt` set, no `choices`)
/// into the chat log, the way retail mirrors event dialogue there. Menus (which
/// carry `choices`) are interactive-only and not logged.
fn emit_event_speech_to_chat(
    event_tx: &broadcast::Sender<AgentEvent>,
    dialog: &crate::state::DialogState,
) {
    if !dialog.choices.is_empty() {
        return;
    }
    let Some(text) = dialog.prompt.as_ref() else {
        return;
    };
    let _ = event_tx.send(AgentEvent::ChatLine {
        line: ChatLine {
            channel: ChatChannel::Say,
            sender: dialog.npc_name.clone().unwrap_or_default(),
            text: text.clone(),
            server_ts: 0,
        },
    });
}

fn emit_event_dialog(
    event_tx: &broadcast::Sender<AgentEvent>,
    dialog: &crate::state::DialogState,
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    name_cache: &std::collections::HashMap<u32, String>,
) {
    let _ = event_tx.send(AgentEvent::EventStart {
        event_id: dialog.event_id,
    });

    let mut dialog = dialog.clone();
    if dialog.npc_name.is_none() {
        dialog.npc_name = name_cache.get(&dialog.npc_id).cloned();
    }
    let _ = event_tx.send(AgentEvent::EventDialog {
        dialog: dialog.clone(),
    });

    pending_event_end.push((dialog.npc_id, dialog.act_index, dialog.event_para));
}

fn decode_chat_std(data: &[u8]) -> Option<ChatLine> {
    const PREFIX: usize = 4 + 15;
    if data.len() < PREFIX {
        return None;
    }
    let kind = data[0];
    let sender = trim_nul_string(&data[4..PREFIX]);
    let text = decode_chat_text(&data[PREFIX..]);
    Some(ChatLine {
        channel: ChatChannel::from_chat_kind(kind),
        sender,
        text,
        server_ts: 0,
    })
}

fn decode_chat_text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    ffxi_proto::autotranslate::decode(&bytes[..end])
}

fn trim_nul_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn build_subpacket_chat(sync: u16, kind: u8, text: &str) -> Vec<u8> {
    let str_bytes = text.as_bytes();
    let str_len = str_bytes.len().min(127);
    let body_unpadded = 2 + str_len + 1;
    let body_padded = (body_unpadded + 3) & !3;
    let total = 4 + body_padded;
    let size_words = (total / 4) as u16;

    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x0B5, size_words, sync));
    buf[4] = kind;

    buf[6..6 + str_len].copy_from_slice(&str_bytes[..str_len]);

    buf
}

fn build_subpacket_tell(sync: u16, recipient: &str, text: &str) -> Vec<u8> {
    let r_bytes = recipient.as_bytes();
    let r_len = r_bytes.len().min(14);
    let t_bytes = text.as_bytes();
    let t_len = t_bytes.len().min(127);

    let body_unpadded = 1 + 1 + 15 + t_len + 1;
    let body_padded = (body_unpadded + 3) & !3;
    let total = 4 + body_padded;
    let size_words = (total / 4) as u16;

    let mut buf = vec![0u8; total];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x0B6, size_words, sync));

    buf[6..6 + r_len].copy_from_slice(&r_bytes[..r_len]);

    buf[21..21 + t_len].copy_from_slice(&t_bytes[..t_len]);
    buf
}

fn build_subpacket_event_end(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    event_num: u16,
    choice: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; 20];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x05B, 5, sync));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..12].copy_from_slice(&choice.to_le_bytes());
    buf[12..14].copy_from_slice(&act_index.to_le_bytes());

    buf[16..18].copy_from_slice(&event_num.to_le_bytes());
    buf[18..20].copy_from_slice(&event_num.to_le_bytes());
    buf
}

pub fn build_subpacket_action(
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

/// c2s 0x110 GP_CLI_COMMAND_FISHING_2 — the mini-game request the client streams while
/// fishing (check-hook, end-game, release, timeout). `mode`/`para`/`para2` follow the
/// LSB validator: vendor/server/src/map/packets/c2s/0x110_fishing_2.{h,cpp}.
pub fn build_subpacket_fishing(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    mode: crate::state::FishingMode,
    para: i32,
    para2: i32,
) -> Vec<u8> {
    let mut buf = vec![0u8; 20];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::FISHING_2,
        5,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..12].copy_from_slice(&para.to_le_bytes());
    buf[12..14].copy_from_slice(&act_index.to_le_bytes());
    buf[14] = mode as u8;
    buf[16..20].copy_from_slice(&para2.to_le_bytes());
    buf
}

pub fn build_subpacket_equip_inspect(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    kind: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x0DD, 4, sync));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());

    buf[8..12].copy_from_slice(&(act_index as u32).to_le_bytes());
    buf[12] = kind;

    buf
}

pub fn build_subpacket_reqlogout(sync: u16, mode: u16, kind: u16) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::REQ_LOGOUT,
        2,
        sync,
    ));
    buf[4..6].copy_from_slice(&mode.to_le_bytes());
    buf[6..8].copy_from_slice(&kind.to_le_bytes());
    buf
}

pub fn build_subpacket_camp(sync: u16, mode: HealMode) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x0E8, 2, sync));
    buf[4..8].copy_from_slice(&mode.as_u32().to_le_bytes());
    buf
}

pub fn build_subpacket_item_use(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    category: u8,
    slot: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 20];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x037, 5, sync));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());

    buf[12..14].copy_from_slice(&act_index.to_le_bytes());
    buf[14] = slot;

    buf[16..20].copy_from_slice(&(category as u32).to_le_bytes());
    buf
}

pub fn build_subpacket_equip_set(
    sync: u16,
    container_index: u8,
    equip_slot: u8,
    container: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::EQUIP_SET,
        2,
        sync,
    ));
    buf[4] = container_index;
    buf[5] = equip_slot;
    buf[6] = container;

    buf
}

fn note_mog_transition(now_in_mog: bool, was: &mut bool, event_tx: &broadcast::Sender<AgentEvent>) {
    if now_in_mog && !*was {
        let _ = event_tx.send(AgentEvent::ChatLine {
            line: crate::state::ChatLine {
                channel: crate::state::ChatChannel::System,
                sender: "<client>".into(),
                text: "You're inside a Mog House (LSB keeps the zone id equal \
                       to the surrounding city). Entity stream is filtered \
                       server-side — use /mhexit to leave."
                    .into(),
                server_ts: 0,
            },
        });
    } else if !now_in_mog && *was {
        let _ = event_tx.send(AgentEvent::ChatLine {
            line: crate::state::ChatLine {
                channel: crate::state::ChatChannel::System,
                sender: "<client>".into(),
                text: "Left the Mog House (server-side `m_moghouseID` cleared).".into(),
                server_ts: 0,
            },
        });
    }
    *was = now_in_mog;
}

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
        in_mog_house: attrs.moghouse_flg != 0,
    }
}

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

    buf
}

fn build_subpacket_maprect_mh_exit(
    sync: u16,
    exit_bit: u8,
    exit_mode: u8,
    x: f32,
    y: f32,
    z: f32,
    act_index: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 24];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x05E, 6, sync));

    buf[4..8].copy_from_slice(b"zmrq");
    buf[8..12].copy_from_slice(&x.to_le_bytes());
    buf[12..16].copy_from_slice(&y.to_le_bytes());
    buf[16..20].copy_from_slice(&z.to_le_bytes());
    buf[20..22].copy_from_slice(&act_index.to_le_bytes());
    buf[22] = exit_bit;
    buf[23] = exit_mode;
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

const MOVE_EMISSION_PERIOD: std::time::Duration = std::time::Duration::from_millis(100);

const MOVE_BIG_JUMP_YALMS: f32 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
enum SelfPosReconcile {
    KeepLocal,

    Rubberband { target: Vec3 },

    Snap,
}

fn reconcile_self_pos(local: Vec3, server: Vec3) -> SelfPosReconcile {
    let dx = server.x - local.x;
    let dy = server.y - local.y;
    let dz = server.z - local.z;
    let dist_sq = dx * dx + dy * dy + dz * dz;

    if dist_sq <= 2.0 * 2.0 {
        SelfPosReconcile::KeepLocal
    } else if dist_sq <= 10.0 * 10.0 {
        SelfPosReconcile::Rubberband { target: server }
    } else {
        SelfPosReconcile::Snap
    }
}

fn lerp_toward(cur: Vec3, target: Vec3, max_step: f32) -> (Vec3, bool) {
    let dx = target.x - cur.x;
    let dy = target.y - cur.y;
    let dz = target.z - cur.z;
    let dist = (dx * dx + dy * dy + dz * dz).sqrt();
    if dist <= max_step || dist <= 1e-4 {
        return (target, true);
    }
    let f = max_step / dist;
    (
        Vec3 {
            x: cur.x + dx * f,
            y: cur.y + dy * f,
            z: cur.z + dz * f,
        },
        false,
    )
}

fn should_emit_pos(
    elapsed: std::time::Duration,
    pos_delta_yalms: f32,
    heading_changed: bool,
) -> bool {
    elapsed >= MOVE_EMISSION_PERIOD || pos_delta_yalms > MOVE_BIG_JUMP_YALMS || heading_changed
}

fn should_break_flood(self_pos_seeded: bool) -> bool {
    self_pos_seeded
}

fn apply_zoneline_spawn_fallback(seed: Vec3, fallback: Option<Vec3>) -> Vec3 {
    const ORIGIN_EPS: f32 = 1.0;
    let near_origin =
        |p: Vec3| p.x.abs() < ORIGIN_EPS && p.y.abs() < ORIGIN_EPS && p.z.abs() < ORIGIN_EPS;
    match fallback {
        Some(fb) if near_origin(seed) && !near_origin(fb) => fb,
        _ => seed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_seed_with_valid_fallback_is_repaired() {
        let dest = v(-16.039, -132.804, -4.217);
        assert_eq!(
            apply_zoneline_spawn_fallback(v(0.0, 0.0, 0.0), Some(dest)),
            dest,
            "origin seed must be replaced by the baked destination"
        );
    }

    #[test]
    fn sane_seed_is_never_overridden() {
        let server = v(573.0, -326.6, -1.1);
        let dest = v(-16.039, -132.804, -4.217);
        assert_eq!(
            apply_zoneline_spawn_fallback(server, Some(dest)),
            server,
            "a non-origin server seed must win over the fallback"
        );
    }

    #[test]
    fn origin_seed_without_fallback_stays_origin() {
        assert_eq!(
            apply_zoneline_spawn_fallback(v(0.0, 0.0, 0.0), None),
            v(0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn origin_fallback_does_not_replace_origin_seed() {
        assert_eq!(
            apply_zoneline_spawn_fallback(v(0.0, 0.0, 0.0), Some(v(0.2, -0.1, 0.0))),
            v(0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn baked_zoneline_resolves_bastok_mines_destination() {
        let to_pos = ffxi_nav::to_pos_for_line(813314682)
            .expect("line 813314682 (S. Gustaberg → Bastok Mines) must exist");
        let dest = v(to_pos[0], to_pos[1], to_pos[2]);
        assert!(
            apply_zoneline_spawn_fallback(v(0.0, 0.0, 0.0), Some(dest)) == dest,
            "baked to_pos {to_pos:?} should be treated as a valid destination"
        );
    }

    const STATIC_TARGID: u16 = 0x123;
    const DYNAMIC_TARGID: u16 = 0x712;

    #[test]
    fn standard_model_with_monster_flag_is_a_mob() {
        for look in [0u16, 5, 6] {
            assert_eq!(
                classify_char_npc(Some(look), STATIC_TARGID, false, true),
                EntityKind::Mob
            );
        }
    }

    #[test]
    fn standard_model_without_monster_flag_is_an_npc() {
        for look in [0u16, 5, 6] {
            assert_eq!(
                classify_char_npc(Some(look), STATIC_TARGID, false, false),
                EntityKind::Npc
            );
        }
    }

    #[test]
    fn dynamic_targid_mob_is_a_mob_without_monster_flag() {
        assert_eq!(
            classify_char_npc(Some(0), DYNAMIC_TARGID, false, false),
            EntityKind::Mob
        );
        assert_eq!(
            classify_char_npc(Some(0), DYNAMIC_TARGID, false, true),
            EntityKind::Mob
        );
    }

    #[test]
    fn pc_owned_standard_model_is_a_pet() {
        assert_eq!(
            classify_char_npc(Some(0), DYNAMIC_TARGID, true, true),
            EntityKind::Pet
        );
    }

    #[test]
    fn equipped_models_are_npcs_and_furniture_is_other() {
        assert_eq!(
            classify_char_npc(Some(1), STATIC_TARGID, false, true),
            EntityKind::Npc
        );
        assert_eq!(
            classify_char_npc(Some(7), STATIC_TARGID, false, false),
            EntityKind::Npc
        );
        for door_size in [2u16, 3, 4] {
            assert_eq!(
                classify_char_npc(Some(door_size), STATIC_TARGID, false, true),
                EntityKind::Other
            );
        }
        assert_eq!(
            classify_char_npc(None, STATIC_TARGID, false, true),
            EntityKind::Other
        );
    }

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    #[test]
    fn should_emit_pos_rate_limits_to_10hz() {
        assert!(!should_emit_pos(
            std::time::Duration::from_millis(50),
            0.1,
            false,
        ));

        assert!(should_emit_pos(
            std::time::Duration::from_millis(100),
            0.0,
            false,
        ));
        assert!(should_emit_pos(
            std::time::Duration::from_millis(120),
            0.0,
            false,
        ));
    }

    #[test]
    fn should_emit_pos_bypasses_rate_limit_on_big_jump() {
        assert!(should_emit_pos(
            std::time::Duration::from_millis(10),
            0.6,
            false,
        ));

        assert!(!should_emit_pos(
            std::time::Duration::from_millis(10),
            0.5,
            false,
        ));
    }

    #[test]
    fn should_emit_pos_bypasses_rate_limit_on_heading_change() {
        assert!(should_emit_pos(
            std::time::Duration::from_millis(10),
            0.0,
            true,
        ));
    }

    #[test]
    fn flood_drain_waits_for_self_pos_seed() {
        assert!(!should_break_flood(false));
        assert!(should_break_flood(true));
    }

    #[test]
    fn cadence_drops_30hz_integrator_to_10hz_emission() {
        let mut last_emit: Option<std::time::Duration> = None;
        let mut now = std::time::Duration::ZERO;
        let mut emits = 0;
        for _ in 0..30 {
            now += std::time::Duration::from_millis(33);
            let elapsed = match last_emit {
                None => std::time::Duration::from_secs(10),
                Some(t) => now - t,
            };
            if should_emit_pos(elapsed, 0.165, false) {
                emits += 1;
                last_emit = Some(now);
            }
        }

        assert!(
            (7..=11).contains(&emits),
            "expected ~10 emissions/s (10 Hz cadence vs 30 Hz integrator), got {emits}",
        );
    }

    #[test]
    fn reconcile_self_pos_keep_local_under_2_yalms() {
        let local = v(0.0, 0.0, 0.0);
        let server = v(1.0, 1.0, 0.5);
        assert_eq!(
            reconcile_self_pos(local, server),
            SelfPosReconcile::KeepLocal,
        );
    }

    #[test]
    fn reconcile_self_pos_rubberband_between_2_and_10() {
        let local = v(0.0, 0.0, 0.0);
        let server = v(3.0, 4.0, 0.0);
        match reconcile_self_pos(local, server) {
            SelfPosReconcile::Rubberband { target } => {
                assert_eq!(target, server);
            }
            other => panic!("expected Rubberband, got {other:?}"),
        }
    }

    #[test]
    fn reconcile_self_pos_snap_above_10_yalms() {
        let local = v(0.0, 0.0, 0.0);
        let server = v(12.0, 5.0, 0.0);
        assert_eq!(reconcile_self_pos(local, server), SelfPosReconcile::Snap,);
    }

    #[test]
    fn reconcile_self_pos_boundaries() {
        let local = v(0.0, 0.0, 0.0);
        let just_inside = v(2.0, 0.0, 0.0);
        assert_eq!(
            reconcile_self_pos(local, just_inside),
            SelfPosReconcile::KeepLocal,
        );

        let edge = v(10.0, 0.0, 0.0);
        assert!(matches!(
            reconcile_self_pos(local, edge),
            SelfPosReconcile::Rubberband { .. },
        ));
    }

    #[test]
    fn lerp_toward_advances_at_capped_step() {
        let (next, reached) = lerp_toward(v(0.0, 0.0, 0.0), v(10.0, 0.0, 0.0), 5.0);
        assert!(!reached);
        assert!((next.x - 5.0).abs() < 1e-3);
    }

    #[test]
    fn lerp_toward_clamps_to_target_on_overshoot() {
        let (next, reached) = lerp_toward(v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), 5.0);
        assert!(reached);
        assert_eq!(next, v(2.0, 0.0, 0.0));
    }

    #[test]
    fn event_end_writes_csid_to_event_para_field() {
        let buf = build_subpacket_event_end(0x1234, 0xDEADBEEF, 0x4242, 535, 0);
        assert_eq!(buf.len(), 20, "header(4) + body(16)");

        assert_eq!(&buf[4..8], &0xDEADBEEFu32.to_le_bytes(), "UniqueNo");
        assert_eq!(&buf[8..12], &0u32.to_le_bytes(), "EndPara (choice=0)");
        assert_eq!(&buf[12..14], &0x4242u16.to_le_bytes(), "ActIndex");
        assert_eq!(&buf[14..16], &0u16.to_le_bytes(), "Mode (End=0)");

        assert_eq!(
            &buf[18..20],
            &535u16.to_le_bytes(),
            "EventPara MUST carry the CSID — LSB validator reads from here",
        );

        assert_eq!(
            &buf[16..18],
            &535u16.to_le_bytes(),
            "EventNum mirrors the CSID for atom0s wire symmetry",
        );
    }

    #[test]
    fn tell_packet_layout_matches_phoenix_struct() {
        let buf = build_subpacket_tell(0xABCD, "Vanari", "hi");
        assert_eq!(buf.len(), 24, "total = 4 hdr + 20 body, padded to mul-of-4");

        let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(id_and_size & 0x1FF, 0x0B6, "opcode");
        assert_eq!(id_and_size >> 9, 6, "size_words");
        let sync = u16::from_le_bytes([buf[2], buf[3]]);
        assert_eq!(sync, 0xABCD, "sync passed through");

        assert_eq!(buf[4], 0, "unknown00");
        assert_eq!(buf[5], 0, "unknown01");
        assert_eq!(&buf[6..12], b"Vanari", "recipient name");
        assert!(buf[12..21].iter().all(|&b| b == 0), "sName NUL-padded");
        assert_eq!(&buf[21..23], b"hi", "message body");
        assert_eq!(buf[23], 0, "trailing NUL");
    }

    #[test]
    fn tell_packet_truncates_oversize_inputs() {
        let long_name = "a".repeat(50);
        let buf = build_subpacket_tell(0, &long_name, "x");

        assert_eq!(&buf[6..20], &[b'a'; 14][..], "first 14 chars of name");
        assert_eq!(buf[20], 0, "sName NUL-terminated even on truncation");
    }

    #[test]
    fn item_use_packet_layout_matches_phoenix_struct() {
        let buf = build_subpacket_item_use(0xBEEF, 0x12345678, 0x0042, 0x00, 7);
        assert_eq!(buf.len(), 20);

        let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(id_and_size & 0x1FF, 0x037, "opcode");
        assert_eq!(id_and_size >> 9, 5, "size_words");
        let sync = u16::from_le_bytes([buf[2], buf[3]]);
        assert_eq!(sync, 0xBEEF, "sync passed through");

        assert_eq!(
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            0x12345678,
            "UniqueNo (recipient)"
        );
        assert_eq!(
            u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            0,
            "ItemNum must be 0 — server-validated (mustEqual 0)"
        );
        assert_eq!(
            u16::from_le_bytes(buf[12..14].try_into().unwrap()),
            0x0042,
            "ActIndex (recipient)"
        );
        assert_eq!(buf[14], 7, "PropertyItemIndex (slot)");
        assert_eq!(buf[15], 0, "padding00");
        assert_eq!(
            u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            0,
            "Category = LOC_INVENTORY"
        );
    }

    #[test]
    fn event_0x032_decodes_full_layout() {
        let mut data = vec![0u8; 16];
        data[0..4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        data[4..6].copy_from_slice(&7u16.to_le_bytes());
        data[6..8].copy_from_slice(&42u16.to_le_bytes());
        data[8..10].copy_from_slice(&3u16.to_le_bytes());
        data[10..12].copy_from_slice(&1u16.to_le_bytes());
        data[12..14].copy_from_slice(&5u16.to_le_bytes());
        data[14..16].copy_from_slice(&9u16.to_le_bytes());

        let d = decode_event_0x032(&data).expect("decoded");
        assert_eq!(d.npc_id, 0x1234_5678);
        assert_eq!(d.act_index, 7);
        assert_eq!(d.event_num, 42);
        assert_eq!(d.event_para, 3);
        assert_eq!(d.mode, 1);
        assert_eq!(d.event_num2, 5);
        assert_eq!(d.event_para2, 9);
        assert!(d.strings.is_empty());
        assert!(d.nums.is_empty());
        assert_eq!(d.event_id, ((0x1234_5678u64 << 16) | 42u64) as u32);
    }

    #[test]
    fn event_0x033_extracts_strings_and_data() {
        let mut data = vec![0u8; 108];
        data[0..4].copy_from_slice(&100u32.to_le_bytes());
        data[4..6].copy_from_slice(&1u16.to_le_bytes());
        data[6..8].copy_from_slice(&50u16.to_le_bytes());

        data[12..16].copy_from_slice(b"Selh");

        data[28..34].copy_from_slice(b"Bastok");

        data[76..80].copy_from_slice(&100i32.to_le_bytes());
        data[80..84].copy_from_slice(&200i32.to_le_bytes());

        let d = decode_event_0x033(&data).expect("decoded");
        assert_eq!(d.strings, vec!["Selh".to_string(), "Bastok".to_string()]);
        assert_eq!(d.nums.len(), 8);
        assert_eq!(d.nums[0], 100);
        assert_eq!(d.nums[1], 200);
        assert_eq!(d.nums[2], 0);
    }

    #[test]
    fn event_0x034_extracts_nums_and_param_block() {
        let mut data = vec![0u8; 48];
        data[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());

        data[4..8].copy_from_slice(&(-5i32).to_le_bytes());
        data[8..12].copy_from_slice(&1234i32.to_le_bytes());

        data[36..38].copy_from_slice(&3u16.to_le_bytes());
        data[38..40].copy_from_slice(&77u16.to_le_bytes());
        data[40..42].copy_from_slice(&2u16.to_le_bytes());
        data[42..44].copy_from_slice(&1u16.to_le_bytes());

        let d = decode_event_0x034(&data).expect("decoded");
        assert_eq!(d.npc_id, 0xDEAD_BEEF);
        assert_eq!(d.act_index, 3);
        assert_eq!(d.event_num, 77);
        assert_eq!(d.event_para, 2);
        assert_eq!(d.mode, 1);
        assert_eq!(d.nums.len(), 8);
        assert_eq!(d.nums[0], -5);
        assert_eq!(d.nums[1], 1234);
    }

    #[test]
    fn battle_message_0x029_substitutes_user_target_amount() {
        use std::collections::HashMap;

        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0x1111_1111u32.to_le_bytes());
        data[4..8].copy_from_slice(&0x2222_2222u32.to_le_bytes());
        data[8..12].copy_from_slice(&12u32.to_le_bytes());
        data[12..16].copy_from_slice(&0u32.to_le_bytes());
        data[16..18].copy_from_slice(&3u16.to_le_bytes());
        data[18..20].copy_from_slice(&4u16.to_le_bytes());
        data[20..22].copy_from_slice(&1u16.to_le_bytes());

        let mut cache = HashMap::new();
        cache.insert(0x1111_1111u32, "Sylvie".to_string());
        cache.insert(0x2222_2222u32, "Mandy".to_string());

        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert_eq!(line.channel, ChatChannel::Battle);
        assert_eq!(line.sender, "Sylvie");
        assert!(line.text.contains("Sylvie"));
        assert!(line.text.contains("Mandy"));
        assert!(line.text.contains("12"));
    }

    #[test]
    fn battle_message_0x02d_uses_reordered_data_offsets() {
        use std::collections::HashMap;

        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&1u32.to_le_bytes());
        data[4..8].copy_from_slice(&2u32.to_le_bytes());
        data[8..10].copy_from_slice(&7u16.to_le_bytes());
        data[10..12].copy_from_slice(&8u16.to_le_bytes());
        data[12..16].copy_from_slice(&999u32.to_le_bytes());
        data[16..20].copy_from_slice(&0u32.to_le_bytes());
        data[20..22].copy_from_slice(&1u16.to_le_bytes());

        let cache = HashMap::new();
        let line = decode_battle_message(&data, &cache, &HashMap::new(), false).expect("decoded");
        assert!(
            line.text.contains("999"),
            "expected amount=999 from offsets [12..16], got: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_falls_back_to_hex_id_for_unknown_actor() {
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        data[4..8].copy_from_slice(&0u32.to_le_bytes());
        data[8..12].copy_from_slice(&5u32.to_le_bytes());
        data[20..22].copy_from_slice(&1u16.to_le_bytes());
        let line =
            decode_battle_message(&data, &HashMap::new(), &HashMap::new(), true).expect("decoded");
        assert_eq!(line.sender, "#DEADBEEF");
        assert!(line.text.contains("<no one>") || line.text.contains("#DEADBEEF"));
    }

    #[test]
    fn battle_message_97_routes_player_to_tar_and_target_to_cas() {
        use std::collections::HashMap;

        let killer_id = 0xAAAA_AAAAu32;
        let victim_id = 0xBBBB_BBBBu32;

        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&killer_id.to_le_bytes());
        data[4..8].copy_from_slice(&victim_id.to_le_bytes());
        data[20..22].copy_from_slice(&97u16.to_le_bytes());

        let mut cache = HashMap::new();
        cache.insert(killer_id, "Orcish_Fodder".to_string());
        cache.insert(victim_id, "Vanari".to_string());

        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");

        assert_eq!(line.sender, "Vanari");

        let v_pos = line.text.find("Vanari").expect("victim in text");
        let o_pos = line.text.find("Orcish_Fodder").expect("killer in text");
        assert!(
            v_pos < o_pos,
            "victim must precede killer in the rendered template, got: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_6_defeats_strips_baked_article_for_pc_subject() {
        use crate::state::EntityKind;
        use std::collections::HashMap;

        let pc_id = 0x0100_0001u32;
        let mob_id = 0x0100_0700u32;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&pc_id.to_le_bytes());
        data[4..8].copy_from_slice(&mob_id.to_le_bytes());
        data[20..22].copy_from_slice(&6u16.to_le_bytes());

        let mut names = HashMap::new();
        names.insert(pc_id, "Atti".to_string());
        names.insert(mob_id, "Tunnel Worm".to_string());
        let mut kinds = HashMap::new();
        kinds.insert(pc_id, EntityKind::Pc);
        kinds.insert(mob_id, EntityKind::Mob);

        let line = decode_battle_message(&data, &names, &kinds, true).expect("decoded");
        assert_eq!(
            line.text, "Atti defeats Tunnel Worm.",
            "PC subject must not carry the baked article, got: {}",
            line.text
        );
        assert!(
            !line.text.starts_with("The "),
            "leading article leaked: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_6_defeats_keeps_article_for_mob_subject() {
        use crate::state::EntityKind;
        use std::collections::HashMap;

        let mob_a = 0x0100_0700u32;
        let mob_b = 0x0100_0701u32;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&mob_a.to_le_bytes());
        data[4..8].copy_from_slice(&mob_b.to_le_bytes());
        data[20..22].copy_from_slice(&6u16.to_le_bytes());

        let mut names = HashMap::new();
        names.insert(mob_a, "Goblin Smithy".to_string());
        names.insert(mob_b, "Forest Hare".to_string());
        let mut kinds = HashMap::new();
        kinds.insert(mob_a, EntityKind::Mob);
        kinds.insert(mob_b, EntityKind::Mob);

        let line = decode_battle_message(&data, &names, &kinds, true).expect("decoded");
        assert_eq!(
            line.text, "The Goblin Smithy defeats Forest Hare.",
            "got: {}",
            line.text
        );
    }

    #[test]
    fn is_fresh_bundle_dedups_retransmits_and_survives_wrap() {
        assert!(is_fresh_bundle(None, 0));
        assert!(is_fresh_bundle(None, 5000));

        assert!(!is_fresh_bundle(Some(42), 42));

        assert!(is_fresh_bundle(Some(42), 43));

        assert!(!is_fresh_bundle(Some(43), 42));

        assert!(is_fresh_bundle(Some(0xFFFF), 0x0001));
        assert!(!is_fresh_bundle(Some(0x0001), 0xFFFF));
    }

    #[test]
    fn battle_message_8_exp_gain_substitutes_hash_marker() {
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[12..16].copy_from_slice(&420u32.to_le_bytes());
        data[16..20].copy_from_slice(&0u32.to_le_bytes());
        data[20..22].copy_from_slice(&8u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "hello".to_string());

        let line = decode_battle_message(&data, &cache, &HashMap::new(), false).expect("decoded");
        assert!(
            line.text.contains("420") && !line.text.contains('#'),
            "expected '#' to be replaced with 420, got: {}",
            line.text
        );
        assert!(line.text.contains("hello"));
    }

    #[test]
    fn battle_message_38_skill_gain_substitutes_skill_and_x() {
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[8..12].copy_from_slice(&48u32.to_le_bytes());
        data[12..16].copy_from_slice(&3u32.to_le_bytes());
        data[20..22].copy_from_slice(&38u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "hello".to_string());
        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");

        assert!(
            line.text.contains("Fishing") && line.text.contains("rises 0.3 points"),
            "expected '<skill>'→Fishing and 'X'→0.3 (decimal), got: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_53_skill_level_up_renders_x_as_integer() {
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[8..12].copy_from_slice(&1u32.to_le_bytes());
        data[12..16].copy_from_slice(&12u32.to_le_bytes());
        data[20..22].copy_from_slice(&53u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "hello".to_string());
        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert!(
            line.text.contains("level 12") && !line.text.contains("1.2"),
            "expected integer level, got: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_253_exp_chain_substitutes_two_hashes_in_order() {
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[12..16].copy_from_slice(&320u32.to_le_bytes());
        data[16..20].copy_from_slice(&5u32.to_le_bytes());
        data[20..22].copy_from_slice(&253u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "hello".to_string());
        let line = decode_battle_message(&data, &cache, &HashMap::new(), false).expect("decoded");
        assert!(
            line.text.contains("chain 5!") && line.text.contains("gains 320"),
            "expected 'chain 5!' and 'gains 320', got: {}",
            line.text
        );
        assert!(
            !line.text.contains('#'),
            "stray '#' remained: {}",
            line.text
        );
    }

    #[test]
    fn substitute_battle_x_marker_respects_token_boundary() {
        let s = substitute_battle_placeholders(
            "reaches level X. BoXing.",
            "cas",
            "tar",
            false,
            false,
            0,
            7,
            53,
            None,
        );
        assert!(s.contains("reaches level 7"), "got: {s}");
        assert!(s.contains("BoXing"), "within-word X must survive, got: {s}");
    }

    #[test]
    fn battle_message_2_magic_damage_resolves_spell_name() {
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0xBEEFu32.to_le_bytes());
        data[8..12].copy_from_slice(&144u32.to_le_bytes());
        data[12..16].copy_from_slice(&0u32.to_le_bytes());
        data[20..22].copy_from_slice(&2u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        cache.insert(0xBEEFu32, "Mandragora".to_string());
        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert!(
            line.text.contains("Daisy")
                && line.text.contains("Mandragora")
                && line.text.contains("Fire")
                && !line.text.contains("<spell>")
                && !line.text.contains("spell #"),
            "expected resolved spell name in: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_565_obtains_gil_override_appends_unit() {
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[8..12].copy_from_slice(&4u32.to_le_bytes());
        data[12..16].copy_from_slice(&0u32.to_le_bytes());
        data[20..22].copy_from_slice(&565u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Mithy".to_string());
        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert_eq!(line.text, "Mithy obtains 4 gil.", "got: {}", line.text);
    }

    #[test]
    fn substitute_status_placeholder_resolves_effect_name() {
        let s = substitute_battle_placeholders(
            "gains the effect of <status>.",
            "cas",
            "tar",
            false,
            false,
            40,
            0,
            186,
            None,
        );
        assert!(
            s.contains("Protect") && !s.contains("<status>") && !s.contains("status #"),
            "expected resolved status name in: {s}"
        );
    }

    #[test]
    fn battle_message_43_readies_weaponskill_substitutes_entity() {
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0u32.to_le_bytes());
        data[8..12].copy_from_slice(&1u32.to_le_bytes());
        data[12..16].copy_from_slice(&0u32.to_le_bytes());
        data[20..22].copy_from_slice(&43u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert!(
            line.text.contains("Daisy readies Hand-to-Hand") && !line.text.contains("<entity>"),
            "expected '<entity>' → Daisy, got: {}",
            line.text
        );
    }

    struct BattleBitWriter {
        data: Vec<u8>,
        pos: usize,
    }

    impl BattleBitWriter {
        fn new(start_bit: usize) -> Self {
            Self {
                data: vec![0u8; 1024],
                pos: start_bit,
            }
        }
        fn write(&mut self, value: u64, bits: u32) {
            let byte_offset = self.pos / 8;
            let bit_in_byte = self.pos % 8;
            let total_bits = bits as usize + bit_in_byte;
            let mask = if bits == 64 {
                u64::MAX
            } else {
                (1u64 << bits) - 1
            };
            let shifted = (value & mask) << bit_in_byte;
            let cover = total_bits.div_ceil(8);
            for i in 0..cover {
                self.data[byte_offset + i] |= ((shifted >> (i * 8)) & 0xFF) as u8;
            }
            self.pos += bits as usize;
        }
        fn into_bytes(self) -> Vec<u8> {
            let used = self.pos.div_ceil(8);
            self.data[..used].to_vec()
        }
    }

    #[test]
    fn battle2_single_hit_emits_damage_line() {
        use std::collections::HashMap;
        let mut w = BattleBitWriter::new(8);
        w.write(0xCAFEu64, 32);
        w.write(1, 6);
        w.write(0, 4);
        w.write(0, 4);
        w.write(0, 32);
        w.write(0, 32);

        w.write(0xBEEFu64, 32);
        w.write(1, 4);

        w.write(0, 3);
        w.write(0, 2);
        w.write(0, 12);
        w.write(0, 5);
        w.write(0, 5);
        w.write(42, 17);
        w.write(1, 10);
        w.write(0, 31);
        w.write(0, 1);
        w.write(0, 1);

        let data = w.into_bytes();
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        cache.insert(0xBEEFu32, "Mandragora".to_string());

        let lines = decode_battle2_action(&data, &cache, &HashMap::new());
        assert_eq!(lines.len(), 1, "expected one line, got: {:?}", lines);
        let l = &lines[0];
        assert_eq!(l.channel, ChatChannel::Battle);
        assert!(
            l.text.contains("Daisy") && l.text.contains("Mandragora") && l.text.contains("42"),
            "expected damage line, got: {}",
            l.text
        );
    }

    #[test]
    fn battle2_magic_damage_substitutes_spell_from_cmd_arg() {
        use std::collections::HashMap;
        let mut w = BattleBitWriter::new(8);
        w.write(0xCAFE, 32);
        w.write(1, 6);
        w.write(0, 4);
        w.write(4, 4);
        w.write(144, 32);
        w.write(0, 32);
        w.write(0xBEEF, 32);
        w.write(1, 4);
        w.write(0, 3);
        w.write(0, 2);
        w.write(0, 12);
        w.write(0, 5);
        w.write(0, 5);
        w.write(87, 17);
        w.write(2, 10);
        w.write(0, 31);
        w.write(0, 1);
        w.write(0, 1);

        let data = w.into_bytes();
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        cache.insert(0xBEEFu32, "Mandragora".to_string());

        let lines = decode_battle2_action(&data, &cache, &HashMap::new());
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert!(
            l.text.contains("Daisy") && l.text.contains("Fire") && l.text.contains("87"),
            "expected casts/Fire/87 in: {}",
            l.text
        );
    }

    #[test]
    fn battle2_starts_casting_resolves_spell_from_param() {
        use std::collections::HashMap;
        let mut w = BattleBitWriter::new(8);
        w.write(0xCAFE, 32);
        w.write(1, 6);
        w.write(0, 4);
        w.write(8, 4);
        w.write(0x68776163, 32);
        w.write(0, 32);
        w.write(0xBEEF, 32);
        w.write(1, 4);
        w.write(0, 3);
        w.write(0, 2);
        w.write(0, 12);
        w.write(0, 5);
        w.write(0, 5);
        w.write(144, 17);
        w.write(327, 10);
        w.write(0, 31);
        w.write(0, 1);
        w.write(0, 1);

        let data = w.into_bytes();
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        cache.insert(0xBEEFu32, "Mandragora".to_string());

        let lines = decode_battle2_action(&data, &cache, &HashMap::new());
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert!(
            l.text.contains("Daisy") && l.text.contains("Fire") && !l.text.contains("spell #"),
            "expected resolved 'Fire' (not a raw spell # fallback) in: {}",
            l.text
        );
    }

    #[test]
    fn battle2_drops_results_with_zero_message_id() {
        use std::collections::HashMap;
        let mut w = BattleBitWriter::new(8);
        w.write(0xCAFE, 32);
        w.write(1, 6);
        w.write(0, 4);
        w.write(0, 4);
        w.write(0, 32);
        w.write(0, 32);
        w.write(0xBEEF, 32);
        w.write(1, 4);
        w.write(0, 3);
        w.write(0, 2);
        w.write(0, 12);
        w.write(0, 5);
        w.write(0, 5);
        w.write(0, 17);
        w.write(0, 10);
        w.write(0, 31);
        w.write(0, 1);
        w.write(0, 1);
        let data = w.into_bytes();
        let lines = decode_battle2_action(&data, &HashMap::new(), &HashMap::new());
        assert!(lines.is_empty(), "expected drop, got: {:?}", lines);
    }

    #[test]
    fn battle2_bitwriter_matches_lsb_pack_byte_layout() {
        let mut w = BattleBitWriter::new(8);
        w.write(0xCAFEu64, 32);
        let bytes = w.into_bytes();
        assert_eq!(bytes[0], 0x00, "workSize slot reserved at byte 0");
        assert_eq!(
            &bytes[1..5],
            &[0xFE, 0xCA, 0x00, 0x00],
            "actor_id LE-packed at byte 1..5 — if this fails, BitWriter \
             no longer matches LSB packBitsBE; do NOT flip BitReader to \
             compensate"
        );
    }

    #[test]
    fn battle2_decoder_pins_worksize_prefix_convention() {
        use std::collections::HashMap;
        let mut w = BattleBitWriter::new(8);
        w.write(0xCAFE, 32);
        w.write(1, 6);
        w.write(0, 4);
        w.write(0, 4);
        w.write(0, 32);
        w.write(0, 32);
        w.write(0xBEEF, 32);
        w.write(1, 4);
        w.write(0, 3);
        w.write(0, 2);
        w.write(0, 12);
        w.write(0, 5);
        w.write(0, 5);
        w.write(42, 17);
        w.write(1, 10);
        w.write(0, 31);
        w.write(0, 1);
        w.write(0, 1);

        let mut data = w.into_bytes();

        let bitstream_bits = data.len() * 8 - 8;
        data[0] = bitstream_bits.div_ceil(8) as u8;

        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        cache.insert(0xBEEFu32, "Mandragora".to_string());

        let lines = decode_battle2_action(&data, &cache, &HashMap::new());
        assert_eq!(
            lines.len(),
            1,
            "wire-shape regression: expected 1 line from a body with 1-byte workSize prefix, got: {:?}",
            lines
        );
        let l = &lines[0];
        assert!(
            l.text.contains("Daisy") && l.text.contains("Mandragora") && l.text.contains("42"),
            "decoded line lost actor/target/damage — check that start-bit 8 is preserved at session.rs:decode_battle2_action; got: {}",
            l.text
        );
    }

    #[test]
    fn battle_message_unknown_id_returns_none() {
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[20..22].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert!(decode_battle_message(&data, &HashMap::new(), &HashMap::new(), true).is_none());
    }

    fn check_message(message_num: u16, data1: u32, data2: u32, cas: u32, tar: u32) -> Vec<u8> {
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&cas.to_le_bytes());
        data[4..8].copy_from_slice(&tar.to_le_bytes());
        data[8..12].copy_from_slice(&data1.to_le_bytes());
        data[12..16].copy_from_slice(&data2.to_le_bytes());
        data[20..22].copy_from_slice(&message_num.to_le_bytes());
        data
    }

    #[test]
    fn check_mob_even_even_renders_difficulty_and_level() {
        use std::collections::HashMap;
        let data = check_message(174, 53, 64 + 4, 1, 2);
        let mut cache = HashMap::new();
        cache.insert(1u32, "Daisy".to_string());
        cache.insert(2u32, "Goblin".to_string());

        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert_eq!(line.sender, "Daisy");
        assert!(
            line.text.contains("Goblin")
                && line.text.contains("Lv. 53")
                && line.text.contains("Even Match"),
            "missing core check fields: {}",
            line.text
        );
        assert!(
            !line.text.to_ascii_lowercase().contains("defense")
                && !line.text.to_ascii_lowercase().contains("evasion"),
            "even/even should suppress def/eva phrase: {}",
            line.text
        );
    }

    #[test]
    fn check_mob_decomposes_def_and_eva_offsets() {
        use std::collections::HashMap;
        let cache: HashMap<u32, String> = [(2u32, "Mob".to_string())].into_iter().collect();
        let cases: &[(u16, Option<&str>, Option<&str>)] = &[
            (170, Some("high defense"), Some("high evasion")),
            (171, None, Some("high evasion")),
            (172, Some("low defense"), Some("high evasion")),
            (173, Some("high defense"), None),
            (174, None, None),
            (175, Some("low defense"), None),
            (176, Some("high defense"), Some("low evasion")),
            (177, None, Some("low evasion")),
            (178, Some("low defense"), Some("low evasion")),
        ];
        for &(msg, def_phrase, eva_phrase) in cases {
            let data = check_message(msg, 25, 64 + 3, 1, 2);
            let line =
                decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
            for (label, phrase) in [("def", def_phrase), ("eva", eva_phrase)] {
                if let Some(p) = phrase {
                    assert!(
                        line.text.contains(p),
                        "msg {msg} missing {label} phrase {p:?}: {}",
                        line.text
                    );
                } else {
                    let unwanted = match label {
                        "def" => "defense",
                        _ => "evasion",
                    };
                    assert!(
                        !line.text.to_ascii_lowercase().contains(unwanted),
                        "msg {msg} should not mention {unwanted}: {}",
                        line.text
                    );
                }
            }
        }
    }

    #[test]
    fn check_mob_renders_all_difficulty_tiers() {
        use std::collections::HashMap;
        let cache: HashMap<u32, String> = [(2u32, "Mob".to_string())].into_iter().collect();
        let tiers = [
            (0u32, "Too Weak"),
            (1, "Incredibly Easy Prey"),
            (2, "Easy Prey"),
            (3, "Decent Challenge"),
            (4, "Even Match"),
            (5, "Tough"),
            (6, "Very Tough"),
            (7, "Incredibly Tough"),
        ];
        for (tier, expected) in tiers {
            let data = check_message(174, 50, 64 + tier, 1, 2);
            let line =
                decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
            assert!(
                line.text.contains(expected),
                "tier {tier} expected {expected:?}: {}",
                line.text
            );
        }
    }

    #[test]
    fn checkparam_renders_acc_att_pairs() {
        use std::collections::HashMap;
        let cache: HashMap<u32, String> = [(1u32, "Daisy".to_string())].into_iter().collect();
        for (msg, label) in [
            (712u16, "Main weapon"),
            (713, "Auxiliary weapon"),
            (714, "Ranged weapon"),
            (715, "Evasion"),
        ] {
            let data = check_message(msg, 321, 654, 1, 1);
            let line =
                decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
            assert!(
                line.text.contains("321") && line.text.contains("654"),
                "msg {msg}: missing numeric pair in {}",
                line.text
            );
            assert!(
                line.text.contains(label),
                "msg {msg}: missing label {label:?} in {}",
                line.text
            );
        }
    }

    #[test]
    fn checkparam_aux_and_ranged_handle_unequipped_slot() {
        use std::collections::HashMap;
        let cache: HashMap<u32, String> = [(1u32, "Daisy".to_string())].into_iter().collect();
        for msg in [713u16, 714] {
            let data = check_message(msg, 0, 0, 1, 1);
            let line =
                decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
            assert!(
                line.text.to_ascii_lowercase().contains("none equipped"),
                "msg {msg} with (0,0) should read \"none equipped\", got: {}",
                line.text
            );
        }
    }

    #[test]
    fn check_impossible_to_gauge_uses_mob_placeholder() {
        use std::collections::HashMap;
        let data = check_message(249, 0, 0, 1, 2);
        let mut cache = HashMap::new();
        cache.insert(1u32, "Daisy".to_string());
        cache.insert(2u32, "King Behemoth".to_string());

        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert!(
            line.text.contains("King Behemoth")
                && line.text.to_ascii_lowercase().contains("impossible"),
            "{}",
            line.text
        );
    }

    #[test]
    fn miscdata_status_icons_drops_placeholder_slots() {
        let mut data = vec![0u8; 4 + 64 + 128];
        data[0..2].copy_from_slice(&0x0009u16.to_le_bytes());

        data[4..6].copy_from_slice(&33u16.to_le_bytes());

        data[6..8].copy_from_slice(&0x00FFu16.to_le_bytes());

        data[8..10].copy_from_slice(&12u16.to_le_bytes());

        let (icons, expiries) = decode_miscdata_status_icons(&data).expect("decoded");
        assert_eq!(icons, vec![33, 12]);
        assert_eq!(expiries.len(), icons.len());
    }

    #[test]
    fn status_icon_expiry_recovers_remaining_seconds() {
        let now_unix = 1_700_000_000u64;
        let vana_now = (now_unix - super::VANA_EPOCH_UNIX) as u32;
        let remaining = 300u32;
        let timestamp = vana_now.wrapping_add(remaining).wrapping_mul(60);
        let expiry = super::status_icon_expiry_unix(timestamp, now_unix);
        assert_eq!(expiry as u64, now_unix + remaining as u64);
        assert_eq!(super::status_icon_expiry_unix(0x7FFF_FFFF, now_unix), 0);
        assert_eq!(super::status_icon_expiry_unix(0, now_unix), 0);
    }

    #[test]
    fn abil_recast_decodes_running_timers() {
        let mut data = vec![0u8; 8 * 31 + 8];
        data[0..2].copy_from_slice(&120u16.to_le_bytes());
        data[3] = 5; // TimerId (Provoke recast group)
        data[8..10].copy_from_slice(&0u16.to_le_bytes()); // second slot ready -> skipped
        data[11] = 7;
        let recasts = super::decode_abil_recast(&data);
        assert_eq!(recasts.len(), 1);
        assert_eq!(recasts[0].0, 5);
        assert!(recasts[0].1 >= now_unix_secs() as u32 + 119);
    }

    #[test]
    fn miscdata_status_icons_rejects_wrong_type() {
        let mut data = vec![0u8; 4 + 64 + 128];
        data[0..2].copy_from_slice(&0x0005u16.to_le_bytes());

        data[4..6].copy_from_slice(&33u16.to_le_bytes());
        assert!(decode_miscdata_status_icons(&data).is_none());
    }

    #[test]
    fn miscdata_status_icons_truncated_returns_none() {
        let data = vec![0u8; 10];
        assert!(decode_miscdata_status_icons(&data).is_none());
    }

    #[test]
    fn shop_list_decodes_rows_and_skips_zero_padding() {
        let mut data = vec![0u8; 4 + 12 * 3];
        data[0..2].copy_from_slice(&5u16.to_le_bytes());

        data[4..8].copy_from_slice(&100u32.to_le_bytes());
        data[8..10].copy_from_slice(&4096u16.to_le_bytes());
        data[10] = 0;

        data[16..20].copy_from_slice(&99999u32.to_le_bytes());
        data[20..22].copy_from_slice(&256u16.to_le_bytes());
        data[22] = 1;

        let shop = decode_shop_list(&data).expect("decoded");
        assert_eq!(shop.offset_index, 5);
        assert_eq!(shop.items.len(), 2);
        assert_eq!(shop.items[0].price, 100);
        assert_eq!(shop.items[0].item_no, 4096);
        assert_eq!(shop.items[1].item_no, 256);
        assert_eq!(shop.items[1].price, 99999);
        assert!(!shop.opened);
    }

    #[test]
    fn shop_buy_packet_layout_matches_server_struct() {
        let buf = build_subpacket_shop_buy(0xABCD, 5, 12, 3);
        assert_eq!(buf.len(), 16);
        let hdr = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(hdr & 0x01FF, 0x083);
        assert_eq!((hdr >> 9) & 0x7F, 4);
        assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), 5, "qty");
        assert_eq!(
            u16::from_le_bytes(buf[8..10].try_into().unwrap()),
            12,
            "shop_no"
        );
        assert_eq!(
            u16::from_le_bytes(buf[10..12].try_into().unwrap()),
            3,
            "shop_index zero-extended to u16"
        );
        assert_eq!(buf[12], 0, "PropertyItemIndex = LOC_INVENTORY");
        assert_eq!(&buf[13..16], &[0u8; 3], "padding");
    }

    #[test]
    fn event_decoders_reject_short_bodies() {
        assert!(decode_event_0x032(&[0u8; 15]).is_none());
        assert!(decode_event_0x033(&[0u8; 107]).is_none());
        assert!(decode_event_0x034(&[0u8; 47]).is_none());
    }

    #[test]
    fn camp_packet_layout_matches_server_struct() {
        for (mode, want) in [
            (HealMode::Toggle, 0u32),
            (HealMode::On, 1),
            (HealMode::Off, 2),
        ] {
            let buf = build_subpacket_camp(0xBEEF, mode);
            assert_eq!(buf.len(), 8, "header (4) + body (4)");
            let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
            assert_eq!(hdr_word & 0x01FF, 0x0E8, "opcode in low 9 bits");
            assert_eq!((hdr_word >> 9) & 0x7F, 2, "size_words=2");
            assert_eq!(
                u16::from_le_bytes([buf[2], buf[3]]),
                0xBEEF,
                "sync echoed in header"
            );
            assert_eq!(
                u32::from_le_bytes(buf[4..8].try_into().unwrap()),
                want,
                "Mode LE for {mode:?}"
            );
        }
    }

    #[test]
    fn gameok_packet_layout_matches_server_struct() {
        let buf = build_subpacket_gameok(0xBEEF);
        assert_eq!(buf.len(), 12, "header (4) + body (8)");
        let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(hdr_word & 0x01FF, 0x00C, "opcode in low 9 bits");
        assert_eq!((hdr_word >> 9) & 0x7F, 3, "size_words=3");
        assert_eq!(
            u16::from_le_bytes([buf[2], buf[3]]),
            0xBEEF,
            "sync echoed in header"
        );
        assert_eq!(
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            0,
            "ClientState must be 0 to pass the server validator"
        );
        assert_eq!(
            u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            0,
            "DebugClientFlg/unused must be 0 to pass the server validator"
        );
    }

    #[test]
    fn equip_inspect_packet_layout_matches_server_struct() {
        let buf = build_subpacket_equip_inspect(0xABCD, 0x1234_5678, 42, 1);
        assert_eq!(buf.len(), 16, "header (4) + body (12)");

        let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(hdr_word & 0x01FF, 0x0DD, "opcode in low 9 bits");
        assert_eq!((hdr_word >> 9) & 0x7F, 4, "size_words=4");
        assert_eq!(
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            0x1234_5678,
            "UniqueNo LE"
        );
        assert_eq!(
            u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            42,
            "ActIndex zero-extended to u32 LE"
        );
        assert_eq!(buf[12], 1, "Kind=CheckName");
        assert_eq!(&buf[13..16], &[0u8; 3], "padding00");
    }

    #[test]
    fn item_use_with_nonzero_category_writes_full_u32() {
        let buf = build_subpacket_item_use(0, 0, 0, 8, 0);
        assert_eq!(
            u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            8,
            "Category u32 LE"
        );
    }

    #[test]
    fn chat_std_decoder_maps_each_channel() {
        use ffxi_proto::map::chat_kind as k;
        let cases = [
            (k::SAY, ChatChannel::Say),
            (k::SHOUT, ChatChannel::Shout),
            (k::TELL, ChatChannel::Tell),
            (k::PARTY, ChatChannel::Party),
            (k::LINKSHELL, ChatChannel::Linkshell),
            (k::YELL, ChatChannel::Yell),
            (k::SYSTEM_1, ChatChannel::System),
            (k::SYSTEM_3, ChatChannel::System),
            (k::EMOTION, ChatChannel::Say),
            (k::NS_PARTY, ChatChannel::Party),
            (k::LINKSHELL2, ChatChannel::Linkshell),
            (200u8, ChatChannel::Other),
        ];
        for (kind, expected) in cases {
            let mut body = vec![0u8; 4 + 15];
            body[0] = kind;
            body.extend_from_slice(b"Hello there");
            body.push(0);
            let line = decode_chat_std(&body).expect("decoder accepts well-formed body");
            assert_eq!(line.channel, expected, "kind {kind} → {expected:?}");
            assert_eq!(line.text, "Hello there");
        }
    }

    #[test]
    fn chat_std_decoder_extracts_sender_and_message() {
        let mut body = vec![0u8; 4 + 15];
        body[0] = ffxi_proto::map::chat_kind::SAY;
        body[4..10].copy_from_slice(b"Sylvie");

        body.extend_from_slice(b"hi all");
        body.push(0);
        let line = decode_chat_std(&body).unwrap();
        assert_eq!(line.sender, "Sylvie");
        assert_eq!(line.text, "hi all");
        assert_eq!(line.channel, ChatChannel::Say);
    }

    #[test]
    fn chat_std_decoder_rejects_truncated_body() {
        assert!(decode_chat_std(&[0u8; 5]).is_none());
        assert!(decode_chat_std(&[0u8; 18]).is_none());
        assert!(decode_chat_std(&[0u8; 19]).is_some());
    }

    #[test]
    fn system_message_substitutes_seconds() {
        let raw = "Executing logout in <seconds> seconds. Cancel healing to remain logged in.";
        let s = substitute_system_placeholders(raw, 30, 0);
        assert_eq!(
            s,
            "Executing logout in 30 seconds. Cancel healing to remain logged in.",
        );
    }

    #[test]
    fn system_message_unknown_id_falls_through() {
        let line = build_system_message_line(decode::SystemMessage {
            para: 7,
            para2: 42,
            message_id: 0xBEEF,
        });
        assert!(line.text.contains("msg #48879"), "{}", line.text);
        assert!(line.text.contains("para=7,42"), "{}", line.text);
        assert!(matches!(line.channel, ChatChannel::System));
    }

    #[test]
    fn system_message_executing_logout_full_line() {
        let line = build_system_message_line(decode::SystemMessage {
            para: 25,
            para2: 0,
            message_id: 7,
        });
        assert!(
            line.text.starts_with("Executing logout in 25 seconds."),
            "{}",
            line.text
        );
        assert!(matches!(line.channel, ChatChannel::System));
    }
}
