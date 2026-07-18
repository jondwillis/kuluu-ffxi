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

/// Lazy loader for the canned-emote chat-text DialogTable (ROM/27/70); when
/// the install lacks it, emote chat degrades to a name-only line.
struct EmoteTextResolver {
    root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,
    table: Option<Option<ffxi_dat::dmsg::EmoteTextDat>>,
}

impl EmoteTextResolver {
    fn new(root: Option<std::sync::Arc<ffxi_dat::DatRoot>>) -> Self {
        Self { root, table: None }
    }

    fn table(&mut self) -> Option<&ffxi_dat::dmsg::EmoteTextDat> {
        let root = self.root.as_ref();
        self.table
            .get_or_insert_with(|| {
                let loaded = root.and_then(|r| ffxi_dat::dmsg::EmoteTextDat::open(r));
                if loaded.is_none() {
                    tracing::info!(
                        "emote text DAT (ROM/27/70) unavailable — emote chat lines degrade to name-only"
                    );
                }
                loaded
            })
            .as_ref()
    }
}

#[derive(Clone, Debug)]
pub enum CharSelection {
    Id(u32),
    Name(String),
}

/// Self Mog House / job state the send loop needs synchronously for the local
/// menus (the folded `SessionState` lives in another task).
#[derive(Debug, Default, Clone, Copy)]
struct SelfMogState {
    myroom: Option<crate::state::MyRoomInfo>,
    mog_zone_flag: bool,
    /// Decoded `LoginState == MYROOM` from the last 0x00A. Kept separate from
    /// `myroom`: a MYROOM login with the `MYROOM_NONE` sentinel model still
    /// spawns at the forced origin and must skip the zoneline seed repair.
    in_myroom: bool,
    mh_2f_unlocked: Option<bool>,
    job_info: Option<crate::state::JobInfoState>,
    /// Per-container capacities from the last 0x01C ITEM_MAX, indexed by LSB
    /// CONTAINER_ID; gates which storage rows the Mog Menu offers.
    container_caps: Option<[u16; decode::ItemMax::CONTAINER_COUNT]>,
    /// Cutscene embedded in the last 0x00A LOGIN ([`decode::ZoneInEvent`]);
    /// consumed by the keepalive loop, which must answer it with 0x05B.
    zone_in_event: Option<decode::ZoneInEvent>,
    /// Last-received (GetItemFlag, LookItemFlag) per key-item table from s2c
    /// 0x055; the c2s 0x064 mark-seen reply must carry the table's full
    /// updated LookItemFlag bitset.
    key_item_tables: [KeyItemTableFlags; decode::ScenarioItem::TABLE_COUNT],
}

/// `received` gates c2s 0x064: before this table's 0x055 arrives the local
/// flags are default-zeroed, and marking seen against them would tell the
/// server (and local state) the table is empty.
#[derive(Debug, Default, Clone, Copy)]
struct KeyItemTableFlags {
    received: bool,
    get_flags: [u32; decode::ScenarioItem::WORDS],
    look_flags: [u32; decode::ScenarioItem::WORDS],
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
    let mut emote_text_resolver = EmoteTextResolver::new(cfg.dat_root.clone());

    let mut name_miss_dedup: std::collections::HashMap<
        (u32, crate::state::NameMissKind),
        std::time::Instant,
    > = Default::default();
    let mut current_zone_id: u16 = 0;

    let mut self_pos = Position::default();

    let mut self_pos_seeded = false;

    let mut flood_in_mog_house = false;

    let mut mog = SelfMogState::default();

    // s2c 0x02A TALKNUMWORK resolves against the zone dialog DAT owned by the
    // keepalive loop's DialogSession, so bodies arriving during the flood are
    // buffered and replayed once it exists — never dropped silently. The cap
    // bounds memory against a misbehaving server; zone onZoneIn lua emits only
    // a handful of messageSpecial lines (e.g.
    // vendor/server/scripts/zones/Attohwa_Chasm/Zone.lua).
    const FLOOD_TALKNUMWORK_MAX: usize = 32;
    let mut flood_talknumwork: Vec<Vec<u8>> = Vec::new();
    while std::time::Instant::now() < flood_deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(500), map.recv_decrypted())
            .await
        {
            Ok(Ok(buf)) => {
                let header = framing::Header::read(&buf[..framing::FFXI_HEADER_SIZE]);
                server_last_seq = header.id_and_size;
                for sub in framing::walk_sub_packets(&buf[framing::FFXI_HEADER_SIZE..]).flatten() {
                    total_subs += 1;
                    if sub.opcode == ffxi_proto::map::s2c::TALKNUMWORK {
                        if flood_talknumwork.len() < FLOOD_TALKNUMWORK_MAX {
                            flood_talknumwork.push(sub.data.to_vec());
                        } else {
                            tracing::warn!("TALKNUMWORK flood buffer full; dropping zone message");
                        }
                        continue;
                    }
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
                        &mut emote_text_resolver,
                        &mut flood_in_mog_house,
                        &mut mog,
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

    let mut sub_seq: u16 = map_client::BOOTSTRAP_SUB_SYNC.wrapping_add(1);

    {
        let payload = build_subpacket_gameok(sub_seq);
        sub_seq = sub_seq.wrapping_add(1);
        map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
            .await?;
        tracing::info!(sub_seq, "sent 0x00C GAMEOK (zone-in)");
    }
    emit_stage(event_tx, Stage::InZone);
    let _ = event_tx.send(AgentEvent::Diagnostics {
        diagnostics: Diagnostics {
            stage: Some(Stage::InZone),
            blowfish_status: Some(BlowfishStatus::Accepted),
            sync_in: Some(server_last_seq),
            sync_out: Some(datagram_header_id(sub_seq)),
            last_server_packet_age_ms: Some(0),
            cert_sha256,
            map_server_addr: Some(map.server_addr().to_string()),
        },
    });

    keepalive_loop(
        map,
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
        emote_text_resolver,
        mog,
        flood_talknumwork,
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

/// Dedup'd warning for sub-packet payloads that fail to decode in
/// [`handle_sub_packet`]: WARN on the first failure per opcode (per process),
/// DEBUG thereafter, so a malformed stream cannot spam the log (kuluu-zkuf).
fn warn_decode_err(opcode: u16, err: impl std::fmt::Display) {
    if first_decode_err(opcode) {
        tracing::warn!(
            opcode = format_args!("{opcode:#06x}"),
            error = %err,
            "sub-packet decode failed; packet dropped \
             (further failures for this opcode logged at DEBUG)"
        );
    } else {
        tracing::debug!(
            opcode = format_args!("{opcode:#06x}"),
            error = %err,
            "sub-packet decode failed; packet dropped"
        );
    }
}

/// True the first time `opcode` is seen (per process) — the dedup gate for
/// [`warn_decode_err`].
fn first_decode_err(opcode: u16) -> bool {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut seen| seen.insert(opcode))
        .unwrap_or(true)
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

    emote_text: &mut EmoteTextResolver,

    was_in_mog_house: &mut bool,

    mog: &mut SelfMogState,

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

                mog.in_myroom = login.myroom.is_some_and(|m| {
                    m.login_state == decode::ServerLoginMyroom::LOGIN_STATE_MYROOM
                });
                mog.myroom = login.myroom.and_then(|m| {
                    m.myroom_model().map(|model| crate::state::MyRoomInfo {
                        model,
                        sub_map: m.sub_map_number,
                        exit_bit: m.exit_bit,
                    })
                });
                mog.mog_zone_flag = login.myroom.is_some_and(|m| m.mog_zone_flag != 0);
                if let Some(ev) = login.zone_in_event {
                    tracing::info!(
                        event_id = ev.event_para,
                        event_zone = ev.event_num,
                        event_mode = ev.event_mode,
                        "0x00A LOGIN carries a zone-in cutscene"
                    );
                    mog.zone_in_event = login.zone_in_event;
                }
                if let Some(m) = login.myroom {
                    match m.login_state {
                        decode::ServerLoginMyroom::LOGIN_STATE_MYROOM => {
                            note_mog_transition(true, was_in_mog_house, event_tx);
                        }
                        decode::ServerLoginMyroom::LOGIN_STATE_GAME => {
                            note_mog_transition(false, was_in_mog_house, event_tx);
                        }
                        _ => {}
                    }
                }

                let _ = event_tx.send(AgentEvent::ZoneChanged {
                    from: None,
                    to: login.zone_no,
                    myroom: mog.myroom,
                    mog_zone_flag: mog.mog_zone_flag,
                });

                if let Some(room) = mog.myroom {
                    let _ = event_tx.send(AgentEvent::EntityUpserted {
                        entity: mh_door_entity(room.model),
                        pos_present: true,
                    });
                }

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
                    let seed_pos = spawn_seed_pos(raw_pos, zoneline_spawn_fallback, mog.in_myroom);
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
                            face_target: head.facetarget(),
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
            if let Ok(head) =
                decode::PosHead::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
                    let seed_pos = spawn_seed_pos(raw_pos, zoneline_spawn_fallback, mog.in_myroom);
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
                        face_target: head.facetarget(),
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
                if let Ok(ent) = decode::EntitySetName::decode(sub.data)
                    .inspect_err(|e| warn_decode_err(sub.opcode, e))
                {
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
                if let Ok(sync) = decode::CharSync::decode(sub.data)
                    .inspect_err(|e| warn_decode_err(sub.opcode, e))
                {
                    // The 2F bit is meaningful only on the SELF sync
                    // (vendor/server/src/map/packets/char_sync.cpp:61).
                    if sync.id == self_char_id {
                        if let Some(unlocked) = sync.mh_2f_unlocked {
                            mog.mh_2f_unlocked = Some(unlocked);
                            let _ = event_tx.send(AgentEvent::MogHouse2fUnlockUpdated { unlocked });
                        }
                    }
                }
            }
            _ => {}
        },
        op if op == s2c::ENTITY_UPDATE2 => {
            if let Ok(pet) =
                decode::PetSync::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
        op if op == s2c::SHOP_SELL => {
            if let Some((price, item_index, count)) = decode_shop_sell(sub.data) {
                let _ = event_tx.send(AgentEvent::ShopSellAppraisal {
                    price,
                    item_index,
                    count,
                });
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
        op if op == s2c::MOTIONMES => {
            if let Ok(m) =
                decode::MotionMes::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::EntityEmoted {
                    actor_id: m.cas_unique_no,
                    actor_index: m.cas_act_index,
                    target_id: m.tar_unique_no,
                    target_index: m.tar_act_index,
                    emote_id: m.mes_num,
                    param: m.param,
                    mode: m.mode,
                });
                // Bell already arrives as Motion ("No emote text for /bell",
                // 0x05a_motionmes.cpp:74), so mode alone gates the text.
                if m.mode != ffxi_proto::map::emote::mode::MOTION {
                    let _ = event_tx.send(AgentEvent::ChatLine {
                        line: emote_chat_line(
                            &m,
                            self_char_id,
                            self_char_name,
                            name_cache,
                            kind_cache,
                            emote_text,
                        ),
                    });
                }
            }
        }
        op if op == s2c::EMOTE_LIST => {
            if let Ok(e) =
                decode::EmoteList::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::EmoteListUpdated {
                    job_bits: e.job_bits,
                    chair_bits: e.chair_bits,
                });
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
            if let Ok(cs) =
                decode::CharStatus::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                if cs.unique_no == self_char_id {
                    // Self speed lands here, not in CHAR_PC — including bind's 0
                    // (vendor/server/scripts/effects/bind.lua setBaseSpeed(0)).
                    self_pos.speed = cs.speed.min(u16::from(u8::MAX)) as u8;
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
            if let Ok(f) =
                decode::FishPacket::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::FishHooked { params: f.into() });
            }
        }
        op if op == s2c::JOB_INFO => {
            if let Ok(ji) =
                decode::JobInfo::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let info = crate::state::JobInfoState::from(ji);
                mog.job_info = Some(info);
                let _ = event_tx.send(AgentEvent::JobInfoUpdated { info });
            }
        }
        op if op == s2c::CLISTATUS => {
            if let Ok(cs) =
                decode::CliStatus::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::CharStatsUpdated {
                    stats: crate::state::CharStatsRaw {
                        hp_max: cs.hp_max,
                        mp_max: cs.mp_max,
                        bp_base: cs.bp_base,
                        bonus: cs.bp_adj,
                        attack: cs.attack,
                        defense: cs.defense,
                        resist: cs.def_elem,
                        ilvl: cs.ilvl,
                    },
                });
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
            if let Ok(fm) =
                decode::ForcedMove::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
            if let Ok(w) = decode::WeatherPacket::decode(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
            if let Ok(ki) = decode::ScenarioItem::decode(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                if let Some(table) = mog.key_item_tables.get_mut(ki.table_index as usize) {
                    *table = KeyItemTableFlags {
                        received: true,
                        get_flags: ki.get_flags,
                        look_flags: ki.look_flags,
                    };
                }
                let _ = event_tx.send(AgentEvent::KeyItemsUpdated {
                    table_index: ki.table_index,
                    ids: ki.owned_key_item_ids(),
                    seen_ids: ki.seen_key_item_ids(),
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
            if let Ok(text) =
                std::str::from_utf8(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
            if let Ok(m) = decode::SystemMessage::decode(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
            if let Ok((attrs, extra)) = decode::PartyAttrs::decode_group_list(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                if attrs.unique_no == self_char_id {
                    note_mog_transition(attrs.moghouse_flg != 0, was_in_mog_house, event_tx);
                }
                let _ = event_tx.send(AgentEvent::PartyMemberUpdated {
                    member: party_member_from_attrs(&attrs, Some(&extra)),
                });
            }
        }
        op if op == s2c::GROUP_ATTR => {
            if let Ok(attrs) = decode::PartyAttrs::decode_group_attr(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                if attrs.unique_no == self_char_id {
                    note_mog_transition(attrs.moghouse_flg != 0, was_in_mog_house, event_tx);
                }
                let _ = event_tx.send(AgentEvent::PartyMemberUpdated {
                    member: party_member_from_attrs(&attrs, None),
                });
            }
        }
        op if op == s2c::ITEM_MAX => {
            if let Ok(m) =
                decode::ItemMax::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
                mog.container_caps = Some(m.capacities);
                let _ = event_tx.send(AgentEvent::InventoryUpdated {
                    container: 0,
                    update: InventoryUpdate::Capacities {
                        capacities: m.capacities.to_vec(),
                    },
                });
            }
        }
        op if op == s2c::ITEM_SAME => {
            if let Ok(s) =
                decode::ItemSame::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                if matches!(s.state, decode::ItemSameState::AllLoaded) {
                    let _ = event_tx.send(AgentEvent::InventoryReady);
                }
            }
        }
        op if op == s2c::ITEM_NUM => {
            if let Ok(n) =
                decode::ItemNum::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
            if let Ok(l) =
                decode::ItemList::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
            if let Ok(a) =
                decode::ItemAttr::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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
            if let Ok(e) =
                decode::EquipList::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::EquipUpdated {
                    slot: e.equip_slot,
                    container: e.container,
                    container_index: e.container_index,
                });
            }
        }
        op if op == s2c::MAGIC_DATA => {
            if let Ok(m) =
                decode::MagicData::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::SpellsKnownUpdated { ids: m.known_ids() });
            }
        }
        op if op == s2c::COMMAND_DATA => {
            if let Ok(c) = decode::CommandData::decode(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
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

// Retail locks movement during events, so there is no upstream value: player
// drift past this (above rubber-band jitter, ~one deliberate step at 5 yalm/s)
// releases a pinned message-dialog as walked-away rather than waiting out the grace.
const EVENT_WALKAWAY_YALMS: f32 = 2.0;

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

/// Route a server-initiated event into the event VM: display it when the VM
/// can drive it (EVENT_END goes out when the script ends), auto-release it
/// otherwise so the char never sticks server-side InEvent (which rejects
/// zonelines, logout, and ~100 other c2s until 0x05B lands).
#[allow(clippy::too_many_arguments)]
fn begin_server_event(
    dialog_session: &mut crate::event_dialog::DialogSession,
    zone_id: u16,
    unique_no: u32,
    act_index: u16,
    event_id: u16,
    name: Option<String>,
    event_tx: &broadcast::Sender<AgentEvent>,
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    auto_event_end: &mut Vec<(u32, u16, u16, u32)>,
) {
    match dialog_session.begin(zone_id, unique_no, act_index, event_id, name) {
        crate::event_dialog::Begin::Frame(dialog) => {
            let _ = event_tx.send(AgentEvent::EventStart {
                event_id: dialog.event_id,
            });
            emit_event_speech_to_chat(event_tx, &dialog);
            let _ = event_tx.send(AgentEvent::EventDialog { dialog });
            pending_event_end.push((unique_no, act_index, event_id));
        }
        crate::event_dialog::Begin::Ended { end_para } => {
            auto_event_end.push((unique_no, act_index, event_id, end_para));
        }
        crate::event_dialog::Begin::Undriveable { stopped_op } => {
            tracing::warn!(
                zone = zone_id,
                unique_no,
                act_index,
                event_id,
                stopped_op = ?stopped_op.map(|op| format!("0x{op:02X}")),
                "auto-releasing VM-undriveable event"
            );
            auto_event_end.push((unique_no, act_index, event_id, 0));
            let _ = event_tx.send(AgentEvent::ChatLine {
                line: ChatLine {
                    channel: ChatChannel::System,
                    sender: "client".into(),
                    text: format!("[event] cutscene {event_id} auto-skipped (not yet supported)"),
                    server_ts: 0,
                },
            });
        }
    }
}

/// Header id (u16 at datagram offset 0) for an outbound bundle, given the
/// next-unused subpacket sync: the sync of the last subpacket placed in the
/// bundle. LSB dispatches a subpacket only when its sync falls in
/// `(client_packet_id, header_id]`, then advances `client_packet_id` to the
/// header (vendor/server/src/map/map_networking.cpp:419-428,471) — a header
/// counter that drifts from the subpacket syncs silently kills the session
/// (subpackets skipped, keepalive/entity flow still healthy). The server's
/// compares are not wrap-aware, so the one datagram straddling u16 wrap
/// (~every 65k sends) is dropped and flow resumes — same loss retail covers
/// by retransmitting unacked subpackets, which we don't implement.
fn datagram_header_id(next_sub_sync: u16) -> u16 {
    next_sub_sync.wrapping_sub(1)
}

#[allow(clippy::too_many_arguments)]
async fn keepalive_loop(
    map: &mut MapClient,
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
    mut emote_text_resolver: EmoteTextResolver,
    mut mog: SelfMogState,
    flood_talknumwork: Vec<Vec<u8>>,
) -> Result<MapOutcome> {
    let mut last_recv = std::time::Instant::now();

    // ITEM_STACK is rate-limited server-side: a second sort of the same container
    // within 1s trips LSB's lightluggage counter and can force-logout the char
    // (vendor/server/src/map/packets/c2s/0x03a_item_stack.cpp:40). Throttle here
    // so mashing the Sort key can never reach that.
    let mut last_item_stack: std::collections::HashMap<u8, std::time::Instant> =
        std::collections::HashMap::new();

    let mut net_health = crate::net_health::NetHealth::new();
    let mut last_net_emit = std::time::Instant::now();
    let mut keepalive_send_failing = false;

    let mut enterzone_seen = false;
    let mut zone_transition_sent = false;

    let mut resrdy_sent = false;

    let mut server_seq_applied: Option<u16> = None;

    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    tick.tick().await;
    let mut reconnect_addr: Option<std::net::SocketAddr> = None;

    let mut reconnect_via_zoneline: Option<u32> = None;
    let mut terminal_disconnect = false;

    let mut pending_maprect: Option<(std::time::Instant, u32)> = None;

    let mut pending_event_end_since: Option<std::time::Instant> = None;
    let mut pending_event_end_anchor: Option<Vec3> = None;

    // Events the VM produced no frame for (frameless completion, unimplemented
    // opcode, missing DAT): EVENT_END goes out on the next send tick so the
    // server doesn't hold the character InEvent behind an empty dialog.
    // (unique_no, act_index, event_id, end_para).
    let mut auto_event_end: Vec<(u32, u16, u16, u32)> = Vec::new();

    let mut dialog_session = crate::event_dialog::DialogSession::new(
        npc_name_resolver.root.clone(),
        character_name.clone(),
    );

    for body in &flood_talknumwork {
        emit_talknumwork_chat(
            body,
            &mut dialog_session,
            current_zone_id,
            &character_name,
            &event_tx,
        );
    }

    let mut local_menu = crate::local_menu::LocalMenuSession::new();

    let mut dbox = crate::delivery_box::DeliveryBoxSession::default();

    let mut is_healing = false;

    let mut last_keepalive_pos: Vec3 = self_pos.pos;

    let mut last_move_emission: Option<std::time::Instant> = None;
    let mut last_emitted_pos: Vec3 = self_pos.pos;
    let mut last_emitted_heading: u8 = self_pos.heading;

    // The targid we broadcast as our head-look (0x015 facetarget) so other clients
    // turn our head. The session only sees the player's selection via target-bearing
    // commands (Action/CheckTarget/UseItem), so track the last one; it stays sticky
    // until the next, and self-heals when the target despawns (renderers can't
    // resolve a stale targid).
    let mut self_face_target: u16 = 0;

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
                        // Local menus first: dismissing one never involves the server.
                        if local_menu.active() {
                            local_menu.clear();
                            let _ = event_tx.send(AgentEvent::EventEnded);
                        // VM-driven event: advance to the next frame, or send
                        // EVENT_END only once the script ends.
                        } else if let Some((u, a, n)) = dialog_session.active_end() {
                            match dialog_session.advance(None) {
                                crate::event_dialog::Advance::Frame(dialog) => {
                                    emit_event_speech_to_chat(&event_tx, &dialog);
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                                crate::event_dialog::Advance::Ended { end_para } => {
                                    let payload = build_subpacket_event_end(sub_seq, u, a, current_zone_id, n, end_para);
                                    sub_seq = sub_seq.wrapping_add(1);
                                    if let Err(e) = map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq).await {
                                        tracing::warn!(error = %e, "EVENT_END (vm) send failed");
                                    }
                                    pending_event_end.retain(|(uid, _, en)| !(*uid == u && *en == n));
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                }
                            }
                        } else if !pending_event_end.is_empty() {
                            let mut payload = Vec::new();
                            for (unique_no, act_index, event_num) in pending_event_end.drain(..) {
                                payload.extend(build_subpacket_event_end(sub_seq, unique_no, act_index, current_zone_id, event_num, 0));
                                sub_seq = sub_seq.wrapping_add(1);
                            }
                            if let Err(e) = map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq).await {
                                tracing::warn!(error = %e, "EVENT_END send failed");
                            }
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
                        // Local menus consume choices before the event VM.
                        if local_menu.active() {
                            match local_menu.advance(Some(choice)) {
                                crate::local_menu::Advance::Frame(dialog) => {
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                                crate::local_menu::Advance::Stub { notice, frame } => {
                                    let _ = event_tx.send(AgentEvent::ChatLine {
                                        line: ChatLine {
                                            channel: ChatChannel::System,
                                            sender: "<client>".into(),
                                            text: format!("[mog] {notice}"),
                                            server_ts: 0,
                                        },
                                    });
                                    let _ = event_tx
                                        .send(AgentEvent::EventDialog { dialog: frame });
                                }
                                crate::local_menu::Advance::Exit(kind) => {
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                    send_mog_house_exit(
                                        map,
                                        kind,
                                        self_pos,
                                        self_act_index,
                                        &mut sub_seq,
                                        server_last_seq,
                                        &mut pending_maprect,
                                        &event_tx,
                                    )
                                    .await;
                                }
                                crate::local_menu::Advance::ChangeJob { main_job, sub_job } => {
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                    send_myroom_job(
                                        map,
                                        main_job,
                                        sub_job,
                                        &mut sub_seq,
                                        server_last_seq,
                                        &event_tx,
                                    )
                                    .await;
                                }
                                // Storage bags are browsed client-side (the server
                                // already streamed every container); the native
                                // viewer opens its Items window from the same choice.
                                crate::local_menu::Advance::OpenStorage { container } => {
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                    let name = ffxi_proto::map::container::name(container)
                                        .unwrap_or("storage");
                                    let _ = event_tx.send(AgentEvent::ChatLine {
                                        line: ChatLine {
                                            channel: ChatChannel::System,
                                            sender: "<client>".into(),
                                            text: format!("[mog] Browsing {name}."),
                                            server_ts: 0,
                                        },
                                    });
                                }
                                crate::local_menu::Advance::DeliveryOpen { box_no } => {
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                    let op = dbox.request_open(box_no, true);
                                    send_pbx(map, &op, &mut sub_seq, server_last_seq, &event_tx).await;
                                }
                                crate::local_menu::Advance::DeliveryTake { box_no: _, slot } => {
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                    let op = dbox.request_take(slot);
                                    send_pbx(map, &op, &mut sub_seq, server_last_seq, &event_tx).await;
                                }
                                crate::local_menu::Advance::Delivery { op } => {
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                    send_pbx(map, &op, &mut sub_seq, server_last_seq, &event_tx).await;
                                }
                                crate::local_menu::Advance::Close => {
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                }
                            }
                        // VM-driven event: feed the selection to the script and
                        // advance; only send EVENT_END once it ends.
                        } else if let Some((u, a, n)) = dialog_session.active_end() {
                            match dialog_session.advance(Some(choice)) {
                                crate::event_dialog::Advance::Frame(dialog) => {
                                    emit_event_speech_to_chat(&event_tx, &dialog);
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                                crate::event_dialog::Advance::Ended { end_para } => {
                                    let payload = build_subpacket_event_end(sub_seq, u, a, current_zone_id, n, end_para);
                                    sub_seq = sub_seq.wrapping_add(1);
                                    if let Err(e) = map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq).await {
                                        tracing::warn!(error = %e, "EVENT_END (vm choice) send failed");
                                    }
                                    pending_event_end.retain(|(uid, _, en)| !(*uid == u && *en == n));
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                }
                            }
                        } else {
                            let payload = build_subpacket_event_end(
                                sub_seq,
                                event_id,
                                act_index,
                                current_zone_id,
                                event_num,
                                choice,
                            );
                            sub_seq = sub_seq.wrapping_add(1);
                            if let Err(e) = map
                                .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                                .await
                            {
                                tracing::warn!(error = %e, "EVENT_END (choice) send failed");
                            }

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
                            "reqlogout send (0x0E7)"
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "reqlogout send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("reqlogout send: {e}"),
                            });
                        }
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
                                sync_out: Some(datagram_header_id(sub_seq)),
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
                            payload_bytes = payload.len(),
                            "chat send (0x0B5)"
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "chat send failed");
                        }
                    }
                    Some(AgentCommand::Tell { to, text }) => {
                        let payload = build_subpacket_tell(sub_seq, &to, &text);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "tell send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("tell send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::Action {
                        target_id,
                        target_index,
                        kind,
                    }) => {
                        // The MH exit door is client-synthesized (LSB spawns no door
                        // NPC) — never let an action on it reach the wire.
                        if target_id == crate::local_menu::MH_DOOR_ENTITY_ID {
                            if matches!(kind, crate::state::ActionKind::Talk)
                                && dialog_session.active_end().is_none()
                            {
                                if let Some(myroom) = mog.myroom {
                                    let dialog =
                                        local_menu.open_mh_exit(&myroom, mog.mh_2f_unlocked);
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                            }
                            continue;
                        }
                        self_face_target = face_target_for(target_index, self_act_index);
                        let payload =
                            build_subpacket_action(sub_seq, target_id, target_index, &kind);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "action send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("action send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::Emote {
                        emote_id,
                        mode,
                        param,
                        target_id,
                        target_index,
                    }) => {
                        // Mirror of the LSB validator (0x05d_motion.cpp
                        // validate + bell note range): a send the server would
                        // drop silently is refused client-side with a reason.
                        let in_event =
                            dialog_session.active_end().is_some() || !pending_event_end.is_empty();
                        if let Some(reason) = emote_send_block_reason(emote_id, mode, param, in_event)
                        {
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("emote not sent: {reason}"),
                            });
                            continue;
                        }
                        let target_index = target_index.unwrap_or(0);
                        self_face_target = face_target_for(target_index, self_act_index);
                        let payload = build_subpacket_motion(
                            sub_seq,
                            target_id.unwrap_or(0),
                            target_index,
                            emote_id,
                            mode,
                            param,
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "emote send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("emote send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::RequestEmoteList) => {
                        let payload = build_subpacket_emote_list_req(sub_seq);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "emote_list request send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("emote_list send: {e}"),
                            });
                        }
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
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "fishing request send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("fishing send: {e}"),
                            });
                        }
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
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "homepoint_return send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("homepoint_return send: {e}"),
                            });
                        }
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
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "shop_buy send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("buy send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::ShopSellReq {
                        qty,
                        item_no,
                        item_index,
                    }) => {
                        let payload =
                            build_subpacket_shop_sell_req(sub_seq, qty, item_no, item_index);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "shop_sell_req send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("sell appraise send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::ShopSellConfirm) => {
                        let payload = build_subpacket_shop_sell_set(sub_seq);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "shop_sell_set send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("sell confirm send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::CheckTarget {
                        target_id,
                        target_index,
                        kind,
                    }) => {
                        self_face_target = face_target_for(target_index, self_act_index);
                        let payload = build_subpacket_equip_inspect(
                            sub_seq,
                            target_id,
                            target_index,
                            kind.as_u8(),
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "equip_inspect send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("check send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::Heal { mode }) => {
                        let payload = build_subpacket_camp(sub_seq, mode);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
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
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "equip_set send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("equip_set send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::StackInventory { container }) => {
                        if !item_stack_allowed(
                            &mut last_item_stack,
                            container,
                            std::time::Instant::now(),
                        ) {
                            tracing::info!(
                                container,
                                "item_stack throttled (<1.1s) to avoid lightluggage kick"
                            );
                        } else {
                            let payload = build_subpacket_item_stack(sub_seq, container);
                            sub_seq = sub_seq.wrapping_add(1);
                            if let Err(e) = map
                                .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                                .await
                            {
                                tracing::warn!(error = %e, "item_stack send failed");
                                let _ = event_tx.send(AgentEvent::Error {
                                    message: format!("item_stack send: {e}"),
                                });
                            }
                        }
                    }
                    Some(AgentCommand::DeliveryBox { op }) => {
                        // Track menu_driven=false so agent-driven flows don't
                        // re-render dialog menus on settle.
                        match op {
                            crate::state::DeliveryBoxOp::PostOpen => {
                                dbox.request_open(crate::state::DeliveryBoxNo::Incoming, false);
                            }
                            crate::state::DeliveryBoxOp::DeliOpen => {
                                dbox.request_open(crate::state::DeliveryBoxNo::Outgoing, false);
                            }
                            _ => {}
                        }
                        send_pbx(map, &op, &mut sub_seq, server_last_seq, &event_tx).await;
                    }
                    Some(AgentCommand::MoveItem {
                        quantity,
                        from_container,
                        to_container,
                        from_slot,
                        to_slot,
                    }) => {
                        let payload = build_subpacket_item_move(
                            sub_seq,
                            quantity,
                            from_container,
                            to_container,
                            from_slot,
                            to_slot,
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "item_move send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("item_move send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::UseItem {
                        container,
                        slot,
                        item_no: _,
                        target_id,
                        target_index,
                    }) => {
                        self_face_target = face_target_for(target_index, self_act_index);
                        let payload = build_subpacket_item_use(
                            sub_seq,
                            target_id,
                            target_index,
                            container,
                            slot,
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "use_item send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("use_item send: {e}"),
                            });
                        }
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
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "MAPRECT send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("MAPRECT send: {e}"),
                            });
                        } else {
                            pending_maprect = Some((std::time::Instant::now(), line_id));
                        }
                    }
                    Some(AgentCommand::MogHouseExit { kind }) => {
                        send_mog_house_exit(
                            map,
                            kind,
                            self_pos,
                            self_act_index,
                            &mut sub_seq,
                            server_last_seq,
                            &mut pending_maprect,
                            &event_tx,
                        )
                        .await;
                    }
                    Some(AgentCommand::ChangeJob { main_job, sub_job }) => {
                        // Never update job state optimistically: LSB validation
                        // failures are silent drops; state refreshes via the
                        // follow-up 0x01B/0x061/0x0DF burst.
                        send_myroom_job(
                            map,
                            main_job,
                            sub_job,
                            &mut sub_seq,
                            server_last_seq,
                            &event_tx,
                        )
                        .await;
                    }
                    Some(AgentCommand::MarkKeyItemsSeen { table_index, ids }) => {
                        // LSB blockedBy(InEvent) rejects 0x064 while an event is
                        // open, and mustEqual(UniqueNo/ActIndex) silently drops a
                        // wrong self id (0x064_scenarioitem.cpp) — skip rather
                        // than burn a seq slot on a silent drop; the unseen state
                        // stays and a later menu close retries.
                        let in_event = dialog_session.active_end().is_some()
                            || !pending_event_end.is_empty();
                        match mog.key_item_tables.get_mut(table_index as usize) {
                            None => {
                                tracing::warn!(
                                    table_index,
                                    "key-item mark-seen for out-of-range table"
                                );
                            }
                            Some(table) => match mark_seen_send_block_reason(
                                in_event,
                                self_act_index,
                                table.received,
                            ) {
                                Err(reason) => {
                                    tracing::debug!(
                                        table_index,
                                        reason,
                                        "skipping key-item mark-seen"
                                    );
                                }
                                Ok(act_index) => {
                                    let mut new_look = table.look_flags;
                                    if fold_seen_ids_into_look_flags(
                                        table_index,
                                        &ids,
                                        &mut new_look,
                                    ) {
                                        let payload = build_subpacket_scenario_item(
                                            sub_seq,
                                            self_char_id,
                                            act_index,
                                            table_index,
                                            &new_look,
                                        );
                                        sub_seq = sub_seq.wrapping_add(1);
                                        match map
                                            .send_encrypted(
                                                &payload,
                                                datagram_header_id(sub_seq),
                                                server_last_seq,
                                            )
                                            .await
                                        {
                                            // LSB sends no 0x055 echo for 0x064;
                                            // fold the new seen bits into local
                                            // state only after a successful send
                                            // so a failed send leaves them unseen
                                            // and a retry re-sends.
                                            Ok(()) => {
                                                table.look_flags = new_look;
                                                let _ = event_tx.send(
                                                    AgentEvent::KeyItemsUpdated {
                                                        table_index,
                                                        ids:
                                                            decode::ScenarioItem::ids_from_flags(
                                                                table_index,
                                                                &table.get_flags,
                                                            ),
                                                        seen_ids:
                                                            decode::ScenarioItem::ids_from_flags(
                                                                table_index,
                                                                &table.look_flags,
                                                            ),
                                                    },
                                                );
                                            }
                                            Err(e) => {
                                                tracing::warn!(error = %e, "key-item mark-seen send failed");
                                            }
                                        }
                                    }
                                }
                            },
                        }
                    }
                    Some(AgentCommand::OpenMogMenu) => {
                        if dialog_session.active_end().is_none() {
                            // Soft warning only: MISC_MOGMENU zones (nomad moogles)
                            // are legal and the client cannot see that zone flag.
                            if mog.myroom.is_none() && !mog.mog_zone_flag {
                                let _ = event_tx.send(AgentEvent::ChatLine {
                                    line: ChatLine {
                                        channel: ChatChannel::System,
                                        sender: "<client>".into(),
                                        text: "Mog Menu opened outside a Mog House — the \
                                               server silently drops job changes unless \
                                               this zone allows the Mog Menu."
                                            .into(),
                                        server_ts: 0,
                                    },
                                });
                            }
                            let dialog = local_menu
                                .open_mog_menu(mog.job_info, mog.container_caps.as_ref().map(|c| c.as_slice()));
                            let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                        }
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
                            .snapshot(last_recv.elapsed(), datagram_header_id(sub_seq));
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
                                     (no server response in 3s). A pending server event \
                                     blocks 0x05E — /endevent or /release to clear it."
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
                        pending_event_end_anchor = Some(self_pos.pos);
                    }
                    (true, true) => {
                        pending_event_end_since = None;
                        pending_event_end_anchor = None;
                    }
                    _ => {}
                }
                let watchdog_fires = pending_event_end_since
                    .map(|t| t.elapsed() > PENDING_EVENT_END_GRACE)
                    .unwrap_or(false);
                let walk_dist = pending_event_end_anchor.map(|anchor| {
                    let dx = self_pos.pos.x - anchor.x;
                    let dy = self_pos.pos.y - anchor.y;
                    let dz = self_pos.pos.z - anchor.z;
                    (dx * dx + dy * dy + dz * dz).sqrt()
                });
                let walked_away = should_release_on_walkaway(user_driven_events, walk_dist);

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

                if zone_transition_sent {
                    if let Some(ev) = mog.zone_in_event.take() {
                        begin_server_event(
                            &mut dialog_session,
                            ev.event_num,
                            self_char_id,
                            self_act_index.unwrap_or(0),
                            ev.event_para,
                            None,
                            &event_tx,
                            &mut pending_event_end,
                            &mut auto_event_end,
                        );
                    }
                }
                for (unique_no, act_index, event_num, end_para) in auto_event_end.drain(..) {
                    payload.extend(build_subpacket_event_end(
                        sub_seq,
                        unique_no,
                        act_index,
                        current_zone_id,
                        event_num,
                        end_para,
                    ));
                    sub_seq = sub_seq.wrapping_add(1);
                }

                // Inside the Mog House LSB spawns the Moogle NPC only in response
                // to c2s 0x01A SendResRdy (SpawnConditionalNPCs, vendor/server/src/
                // map/packets/c2s/0x01a_action.cpp:449-461) — the 0x015 pos path
                // that spawns city NPCs early-returns when inMogHouse. Outside the
                // MH the same action pre-warms NPC/MOB/TRUST spawn lists.
                if zone_transition_sent && self_pos_seeded && !resrdy_sent {
                    resrdy_sent = true;
                    payload.extend(build_subpacket_action(
                        sub_seq,
                        self_char_id,
                        self_act_index.unwrap_or(0),
                        &crate::state::ActionKind::SendResRdy,
                    ));
                    sub_seq = sub_seq.wrapping_add(1);
                    tracing::info!("sent 0x01A SendResRdy (post zone-in spawn request)");
                }

                if (!user_driven_events || watchdog_fires || walked_away)
                    && !pending_event_end.is_empty()
                {
                    for (unique_no, act_index, event_num) in pending_event_end.drain(..) {
                        payload.extend(build_subpacket_event_end(
                            sub_seq,
                            unique_no,
                            act_index,
                            current_zone_id,
                            event_num,
                            0,
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
                    } else if walked_away {
                        tracing::info!(
                            moved_yalms = walk_dist.unwrap_or(0.0),
                            "released pinned event: player walked away from dialog"
                        );
                    }
                    pending_event_end_since = None;
                    pending_event_end_anchor = None;
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
                        self_face_target,
                    ));
                    sub_seq = sub_seq.wrapping_add(1);
                    last_keepalive_pos = self_pos.pos;
                    last_emitted_pos = self_pos.pos;
                    last_emitted_heading = self_pos.heading;
                    last_move_emission = Some(std::time::Instant::now());
                }

                if !payload.is_empty() {
                    match map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq).await {
                        Ok(()) => {
                            if keepalive_send_failing {
                                keepalive_send_failing = false;
                                tracing::info!("keepalive send recovered");
                            }
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
                                        myroom: None,
                                        mog_zone_flag: false,
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

                            if sub.opcode == ffxi_proto::map::s2c::OPENMOGMENU {
                                // Server events own the dialog surface; LSB also
                                // blocks 0x05E/0x100 while InEvent.
                                if dialog_session.active_end().is_none() {
                                    let dialog = local_menu
                                        .open_mog_menu(mog.job_info, mog.container_caps.as_ref().map(|c| c.as_slice()));
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                                continue;
                            }

                            if sub.opcode == ffxi_proto::map::s2c::PBX_RESULT {
                                match decode::PbxResult::decode(sub.data) {
                                    Ok(r) => {
                                        let out = dbox.on_result(&r);
                                        for (box_no, update) in out.updates {
                                            let _ = event_tx.send(
                                                AgentEvent::DeliveryBoxUpdated { box_no, update },
                                            );
                                        }
                                        for text in out.notices {
                                            let _ = event_tx.send(AgentEvent::ChatLine {
                                                line: ChatLine {
                                                    channel: ChatChannel::System,
                                                    sender: "<client>".into(),
                                                    text: format!("[delivery] {text}"),
                                                    server_ts: 0,
                                                },
                                            });
                                        }
                                        for op in &out.sends {
                                            send_pbx(
                                                map,
                                                op,
                                                &mut sub_seq,
                                                server_last_seq,
                                                &event_tx,
                                            )
                                            .await;
                                        }
                                        if out.settled && dbox.menu_driven() {
                                            let dialog = match dbox.open() {
                                                Some(box_no) => local_menu
                                                    .open_delivery_box(box_no, dbox.slots()),
                                                None => local_menu.open_delivery_submenu(),
                                            };
                                            let _ = event_tx
                                                .send(AgentEvent::EventDialog { dialog });
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = ?e, "could not decode 0x04B PBX_RESULT");
                                    }
                                }
                                continue;
                            }

                            // TALKNUMWORK resolves against the zone dialog DAT
                            // that dialog_session owns, so it can't live in
                            // handle_sub_packet.
                            if sub.opcode == ffxi_proto::map::s2c::TALKNUMWORK {
                                emit_talknumwork_chat(
                                    sub.data,
                                    &mut dialog_session,
                                    current_zone_id,
                                    &character_name,
                                    &event_tx,
                                );
                                continue;
                            }

                            // Event triggers (0x32/0x33/0x34) route through the
                            // event VM, never the legacy raw dialog.
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
                                    begin_server_event(
                                        &mut dialog_session,
                                        current_zone_id,
                                        unique_no,
                                        act_index,
                                        event_id,
                                        name,
                                        &event_tx,
                                        &mut pending_event_end,
                                        &mut auto_event_end,
                                    );
                                    continue;
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
                                &mut emote_text_resolver,
                                &mut self_in_mog_house,
                                &mut mog,
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
            // send_if_modified: only signal watch receivers when the event
            // actually mutated the folded state, so per-frame no-op events
            // (e.g. identical PositionChanged / EntityUpserted resends) do not
            // trigger downstream scene rebuilds (kuluu-p09).
            Ok(event) => {
                state_tx.send_if_modified(|s| s.apply_event(&event));
            }
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

/// Decode one s2c 0x02A TALKNUMWORK body and emit its chat line. Shared by the
/// keepalive receive path and the zone-in flood replay so both honor the
/// never-drop-silently invariant of [`talknumwork_chat_line`].
fn emit_talknumwork_chat(
    body: &[u8],
    dialog_session: &mut crate::event_dialog::DialogSession,
    zone_id: u16,
    character_name: &str,
    event_tx: &broadcast::Sender<AgentEvent>,
) {
    match decode::TalkNumWork::decode(body) {
        Ok(tnw) => {
            let zone_text = dialog_session.zone_text(zone_id, tnw.message_index() as usize);
            let _ = event_tx.send(AgentEvent::ChatLine {
                line: talknumwork_chat_line(&tnw, zone_text, character_name),
            });
        }
        Err(e) => warn_decode_err(ffxi_proto::map::s2c::TALKNUMWORK, &e),
    }
}

/// Render s2c 0x02A TALKNUMWORK as a chat line: the zone dialog DAT entry at
/// `message_index()` with `num[]` params substituted ({Num:N}, {KeyItem:N},
/// {Item:N}). Degrades to a placeholder when the zone's string DAT is
/// unavailable — the message must never drop silently.
fn talknumwork_chat_line(
    tnw: &decode::TalkNumWork,
    zone_text: Option<String>,
    player_name: &str,
) -> ChatLine {
    let speaker = (!tnw.hide_name()).then(|| tnw.speaker_name()).flatten();
    let text = match zone_text {
        Some(raw) => crate::event_dialog::substitute_entity_names(
            crate::event_dialog::substitute_nums(
                crate::event_dialog::substitute_names(
                    ffxi_event::clean_display(&raw, &tnw.num),
                    player_name,
                    speaker.as_deref(),
                ),
                &tnw.num,
            ),
            &tnw.num,
        ),
        None => format!(
            "[zone message {} — dialog DAT unavailable; params {:?}]",
            tnw.message_index(),
            tnw.num,
        ),
    };
    ChatLine {
        channel: if speaker.is_some() {
            ChatChannel::Say
        } else {
            ChatChannel::System
        },
        sender: speaker.unwrap_or_default(),
        text,
        server_ts: 0,
    }
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

/// Compose the chat line for a 0x05A MOTIONMES from the client-side emote
/// DialogTable, third-person for everyone (self wording is a retail unknown,
/// bead kuluu-d4u). Falls back to a name-only line when the DAT is absent or
/// lacks the entry.
fn emote_chat_line(
    m: &decode::MotionMes,
    self_char_id: u32,
    self_char_name: &str,
    name_cache: &std::collections::HashMap<u32, String>,
    kind_cache: &std::collections::HashMap<u32, crate::state::EntityKind>,
    emote_text: &mut EmoteTextResolver,
) -> ChatLine {
    let name_of = |id: u32| -> String {
        if id == self_char_id {
            self_char_name.to_string()
        } else {
            name_for_id(id, name_cache)
        }
    };
    let cas_name = name_of(m.cas_unique_no);
    let targeted = m.targeted();
    let tar_name = targeted.then(|| name_of(m.tar_unique_no));
    // The "[the /]" article: NPCs/mobs keep "the ", player characters drop it.
    let target_article = targeted && !is_pc(m.tar_unique_no, kind_cache);
    let text = emote_text
        .table()
        .and_then(|t| {
            t.line(
                m.mes_num,
                targeted,
                &ffxi_dat::dmsg::EmoteLineContext {
                    caster: &cas_name,
                    target: tar_name.as_deref(),
                    target_article,
                },
            )
        })
        .unwrap_or_else(|| fallback_emote_text(&cas_name, m.mes_num));
    ChatLine {
        channel: ChatChannel::Emote,
        sender: String::new(),
        text,
        server_ts: 0,
    }
}

fn fallback_emote_text(cas_name: &str, mes_num: u16) -> String {
    use ffxi_proto::map::emote::{JOB_MESNUM_BASE, JOB_MESNUM_MAX};
    let command = if (JOB_MESNUM_BASE..=JOB_MESNUM_MAX).contains(&mes_num) {
        "jobemote".to_string()
    } else {
        u8::try_from(mes_num)
            .ok()
            .and_then(ffxi_proto::emote_names::lookup)
            .map(str::to_lowercase)
            .unwrap_or_else(|| format!("emote{mes_num}"))
    };
    format!("{cas_name} uses /{command}.")
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

// GP_SERV_COMMAND_SHOP_SELL, vendor/server/src/map/packets/s2c/0x03d_shop_sell.h:
// Price u32, PropertyItemIndex u8, Type u8, padding u16, Count u32. LSB only emits it
// as the SHOP_SELL_REQ appraisal answer (Type = 0, 0x03d_shop_sell.cpp); a completed
// sale is announced via GP_SERV_COMMAND_MESSAGE + ITEM_SAME instead
// (0x085_shop_sell_set.cpp process). Returns (price, item_index, count).
fn decode_shop_sell(data: &[u8]) -> Option<(u32, u8, u32)> {
    const BODY_LEN: usize = 12;
    if data.len() < BODY_LEN {
        return None;
    }
    let price = u32::from_le_bytes(data[0..4].try_into().unwrap());
    let item_index = data[4];
    let count = u32::from_le_bytes(data[8..12].try_into().unwrap());
    Some((price, item_index, count))
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

use ffxi_proto::vana_time::VANA_EPOCH_UNIX;

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

// GP_CLI_COMMAND_SHOP_SELL_REQ, vendor/server/src/map/packets/c2s/0x084_shop_sell_req.h:
// ItemNum u32, ItemNo u16, ItemIndex u8, padding u8. The server appraises the item in
// that LOC_INVENTORY slot, clamps ItemNum to the held quantity, parks it in the shop
// trade container, and answers with s2c 0x03D SHOP_SELL (0x084_shop_sell_req.cpp).
pub fn build_subpacket_shop_sell_req(sync: u16, qty: u32, item_no: u16, item_index: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x084, 3, sync));
    buf[4..8].copy_from_slice(&qty.to_le_bytes());
    buf[8..10].copy_from_slice(&item_no.to_le_bytes());
    buf[10] = item_index;
    buf
}

// GP_CLI_COMMAND_SHOP_SELL_SET, vendor/server/src/map/packets/c2s/0x085_shop_sell_set.h:
// SellFlag u16, padding u16. The server validator rejects the packet unless SellFlag
// equals 1 and a SHOP_SELL_REQ preceded it (0x085_shop_sell_set.cpp validate).
pub fn build_subpacket_shop_sell_set(sync: u16) -> Vec<u8> {
    const SELL_FLAG_CONFIRM: u16 = 1;
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x085, 2, sync));
    buf[4..6].copy_from_slice(&SELL_FLAG_CONFIRM.to_le_bytes());
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

// c2s 0x05B GP_CLI_COMMAND_EVENTEND (vendor/server/src/map/packets/c2s/
// 0x05b_eventend.h:34-41): UniqueNo u32, EndPara u32, ActIndex u16, Mode u16
// (0 = End), EventNum u16 (zone id — retail echoes GP_SERV LOGIN EventNum,
// 0x00a_login.cpp:187), EventPara u16 (the event id the validator matches
// against currentEvent->eventId, validation.cpp:71-76).
fn build_subpacket_event_end(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    event_zone: u16,
    event_id: u16,
    choice: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; 20];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x05B, 5, sync));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..12].copy_from_slice(&choice.to_le_bytes());
    buf[12..14].copy_from_slice(&act_index.to_le_bytes());

    buf[16..18].copy_from_slice(&event_zone.to_le_bytes());
    buf[18..20].copy_from_slice(&event_id.to_le_bytes());
    buf
}

// Inverse of the s2c 0x055 id decode — LSB reads the bits back as
// keyItemId = TableIndex*512 + word*32 + bit (vendor/server/src/map/packets/
// c2s/0x064_scenarioitem.cpp:44). Ids outside `table_index`'s range are
// ignored; returns whether any bit changed.
fn fold_seen_ids_into_look_flags(
    table_index: u16,
    ids: &[u16],
    look_flags: &mut [u32; decode::ScenarioItem::WORDS],
) -> bool {
    let word_bits = u32::BITS as usize;
    let mut changed = false;
    for id in ids {
        let global = *id as usize;
        if global / decode::ScenarioItem::BITS_PER_TABLE != table_index as usize {
            continue;
        }
        let local = global % decode::ScenarioItem::BITS_PER_TABLE;
        let mask = 1u32 << (local % word_bits);
        if look_flags[local / word_bits] & mask == 0 {
            look_flags[local / word_bits] |= mask;
            changed = true;
        }
    }
    changed
}

// LSB gates c2s 0x064 with blockedBy(InEvent) and silently drops it unless
// UniqueNo == char id and ActIndex == self targid (vendor/server/src/map/
// packets/c2s/0x064_scenarioitem.cpp:31-33), so an unseeded targid must skip
// the send; a table whose s2c 0x055 never arrived has only default-zeroed
// local flags, so marking against it would report the table empty. Ok carries
// the validated ActIndex.
fn mark_seen_send_block_reason(
    in_event: bool,
    self_act_index: Option<u16>,
    table_received: bool,
) -> Result<u16, &'static str> {
    if in_event {
        return Err("InEvent blocks 0x064");
    }
    let act_index = self_act_index.ok_or("self act_index not yet seeded")?;
    if !table_received {
        return Err("table's 0x055 not received this session");
    }
    Ok(act_index)
}

// c2s 0x064 GP_CLI_COMMAND_SCENARIOITEM (vendor/server/src/map/packets/c2s/
// 0x064_scenarioitem.h): UniqueNo u32, LookItemFlag u32[16], ActIndex u16,
// TableIndex u16. The server ORs every set LookItemFlag bit into the table's
// seen list and validates UniqueNo == char id, ActIndex == self targid,
// TableIndex < tables.size() (0x064_scenarioitem.cpp); blocked while InEvent.
fn build_subpacket_scenario_item(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    table_index: u16,
    look_flags: &[u32; decode::ScenarioItem::WORDS],
) -> Vec<u8> {
    const TOTAL: usize = 4 + 4 + decode::ScenarioItem::WORDS * 4 + 2 + 2;
    let mut buf = vec![0u8; TOTAL];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::SCENARIO_ITEM,
        (TOTAL / 4) as u16,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    for (i, w) in look_flags.iter().enumerate() {
        let o = 8 + i * 4;
        buf[o..o + 4].copy_from_slice(&w.to_le_bytes());
    }
    buf[72..74].copy_from_slice(&act_index.to_le_bytes());
    buf[74..76].copy_from_slice(&table_index.to_le_bytes());
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

// c2s 0x05D GP_CLI_COMMAND_MOTION: UniqueNo u32 @4, ActIndex u16 @8, Number u8
// @10 (emote id), Mode u8 @11, Param u16 @12, pad u16 @14
// (vendor/server/src/map/packets/c2s/0x05d_motion.h:28-35). Note the c2s Mode
// byte precedes Param, unlike the s2c 0x05A layout.
pub fn build_subpacket_motion(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    number: u8,
    mode: u8,
    param: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MOTION,
        4,
        sync,
    ));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    buf[8..10].copy_from_slice(&act_index.to_le_bytes());
    buf[10] = number;
    buf[11] = mode;
    buf[12..14].copy_from_slice(&param.to_le_bytes());
    buf
}

// c2s 0x119 GP_CLI_COMMAND_EMOTE_LIST — header only
// (vendor/server/src/map/packets/c2s/0x119_emote_list.h declares no payload).
pub fn build_subpacket_emote_list_req(sync: u16) -> Vec<u8> {
    build_subpacket_header(ffxi_proto::map::c2s::EMOTE_LIST, 1, sync).to_vec()
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

// GP_CLI_COMMAND_ITEM_STACK, vendor/server/src/map/packets/c2s/0x03a_item_stack.h:
// `uint32_t Category` (container id) after the 4-byte subpacket header, so 8 bytes
// total (size_words = 2). The server consolidates same-id partial stacks.
pub fn build_subpacket_item_stack(sync: u16, container: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::ITEM_STACK,
        2,
        sync,
    ));
    buf[4..8].copy_from_slice(&(container as u32).to_le_bytes());

    buf
}

// GP_CLI_COMMAND_ITEM_MOVE, vendor/server/src/map/packets/c2s/0x029_item_move.h:
// ItemNum u32 @4, Category1 u8 @8, Category2 u8 @9, ItemIndex1 u8 @10,
// ItemIndex2 u8 @11 — 12 bytes (size_words = 3). An ItemIndex2 < 82 asks for a
// same-id stack merge into that slot; anything larger lets the server pick a free
// slot (0x029_item_move.cpp process), which retail requests with 0xFF.
const ITEM_MOVE_AUTO_SLOT: u8 = 0xFF;

pub fn build_subpacket_item_move(
    sync: u16,
    quantity: u32,
    from_container: u8,
    to_container: u8,
    from_slot: u8,
    to_slot: Option<u8>,
) -> Vec<u8> {
    let mut buf = vec![0u8; 12];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::ITEM_MOVE,
        3,
        sync,
    ));
    buf[4..8].copy_from_slice(&quantity.to_le_bytes());
    buf[8] = from_container;
    buf[9] = to_container;
    buf[10] = from_slot;
    buf[11] = to_slot.unwrap_or(ITEM_MOVE_AUTO_SLOT);
    buf
}

// GP_CLI_COMMAND_PBX, vendor/server/src/map/packets/c2s/0x04d_pbx.h: Command u8
// @4, BoxNo i8 @5, PostWorkNo i8 @6, ItemWorkNo i8 @7, ItemStacks i32 @8,
// Result/ResParam1-3 i8 @12-15 (the validator requires all four zero c2s),
// TargetName[16] @16 — 32 bytes (size_words = 8). Per-command field defaults
// mirror the LSB PacketValidator (0x04d_pbx.cpp validate): unused numeric
// fields are -1; Recv's ItemWorkNo must be 1; Set/Send/Cancel are pinned to the
// Outgoing box, Recv/Accept/Reject to Incoming, Query/Confirm/*Open/Close to
// BoxNo None.
pub fn build_subpacket_pbx(sync: u16, op: &crate::state::DeliveryBoxOp) -> Vec<u8> {
    use crate::state::DeliveryBoxOp as Op;
    use ffxi_proto::map::pbx::{boxno, command};
    type Fields<'a> = (u8, i8, i8, i8, i32, Option<&'a str>);
    let (cmd, box_no, post_work_no, item_work_no, item_stacks, name): Fields = match op {
        Op::Work { box_no } => (command::WORK, box_no.wire(), -1, -1, -1, None),
        Op::Set {
            slot,
            inventory_slot,
            quantity,
            recipient,
        } => (
            command::SET,
            boxno::OUTGOING,
            *slot as i8,
            *inventory_slot as i8,
            *quantity as i32,
            Some(recipient),
        ),
        Op::Send { slot } => (command::SEND, boxno::OUTGOING, *slot as i8, -1, -1, None),
        Op::Cancel { slot } => (command::CANCEL, boxno::OUTGOING, *slot as i8, -1, -1, None),
        Op::Check { box_no } => (command::CHECK, box_no.wire(), -1, -1, -1, None),
        Op::Recv { slot } => (command::RECV, boxno::INCOMING, *slot as i8, 1, -1, None),
        Op::Confirm => (command::CONFIRM, boxno::NONE, -1, -1, -1, None),
        Op::Accept { slot } => (command::ACCEPT, boxno::INCOMING, *slot as i8, -1, -1, None),
        Op::Reject { slot } => (command::REJECT, boxno::INCOMING, *slot as i8, -1, -1, None),
        Op::Get { box_no, slot } => (command::GET, box_no.wire(), *slot as i8, -1, -1, None),
        Op::Clear { box_no, slot } => (command::CLEAR, box_no.wire(), *slot as i8, -1, -1, None),
        Op::Query { recipient } => (command::QUERY, boxno::NONE, -1, -1, -1, Some(recipient)),
        Op::DeliOpen => (command::DELI_OPEN, boxno::NONE, -1, -1, -1, None),
        Op::PostOpen => (command::POST_OPEN, boxno::NONE, -1, -1, -1, None),
        Op::PostClose { .. } => (command::POST_CLOSE, boxno::NONE, -1, -1, -1, None),
    };
    let mut buf = vec![0u8; 32];
    buf[0..4].copy_from_slice(&build_subpacket_header(ffxi_proto::map::c2s::PBX, 8, sync));
    buf[4] = cmd;
    buf[5] = box_no as u8;
    buf[6] = post_work_no as u8;
    buf[7] = item_work_no as u8;
    buf[8..12].copy_from_slice(&item_stacks.to_le_bytes());
    if let Some(name) = name {
        let bytes = name.as_bytes();
        let n = bytes.len().min(15); // NUL terminator stays inside TargetName[16]
        buf[16..16 + n].copy_from_slice(&bytes[..n]);
    }
    buf
}

async fn send_pbx(
    map: &mut MapClient,
    op: &crate::state::DeliveryBoxOp,
    sub_seq: &mut u16,
    server_last_seq: u16,
    event_tx: &broadcast::Sender<AgentEvent>,
) {
    let payload = build_subpacket_pbx(*sub_seq, op);
    tracing::info!(?op, "sending 0x04D PBX");
    *sub_seq = sub_seq.wrapping_add(1);
    if let Err(e) = map
        .send_encrypted(&payload, datagram_header_id(*sub_seq), server_last_seq)
        .await
    {
        tracing::warn!(error = %e, "pbx send failed");
        let _ = event_tx.send(AgentEvent::Error {
            message: format!("delivery box send: {e}"),
        });
    }
}

// LSB kicks a character whose ITEM_STACK requests for one container arrive faster
// than 1/sec (vendor/server/src/map/packets/c2s/0x03a_item_stack.cpp:40); 1.1s
// keeps a margin over that window.
const ITEM_STACK_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1100);

/// Whether an ITEM_STACK for `container` may be sent now. Records `now` as the
/// last-sent time when it returns true, so the throttle is per-container.
fn item_stack_allowed(
    last: &mut std::collections::HashMap<u8, std::time::Instant>,
    container: u8,
    now: std::time::Instant,
) -> bool {
    let too_soon = last
        .get(&container)
        .is_some_and(|t| now.duration_since(*t) < ITEM_STACK_MIN_INTERVAL);
    if !too_soon {
        last.insert(container, now);
    }
    !too_soon
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

/// The RectID fourcc LSB matches for the universal MH exit
/// (vendor/server/src/map/packets/c2s/0x05e_maprect.cpp:72). Emitted by
/// [`build_subpacket_maprect_mh_exit`]; also the `pending_maprect` line id.
const ZMRQ_LE: u32 = u32::from_le_bytes(*b"zmrq");

#[allow(clippy::too_many_arguments)]
async fn send_mog_house_exit(
    map: &mut MapClient,
    kind: crate::state::MogHouseExit,
    self_pos: Position,
    self_act_index: Option<u16>,
    sub_seq: &mut u16,
    server_last_seq: u16,
    pending_maprect: &mut Option<(std::time::Instant, u32)>,
    event_tx: &broadcast::Sender<AgentEvent>,
) {
    let Some(act_index) = self_act_index else {
        let _ = event_tx.send(AgentEvent::Error {
            message: "MogHouseExit before self ActIndex known".into(),
        });
        return;
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
        *sub_seq,
        exit_bit,
        exit_mode,
        self_pos.pos.x,
        self_pos.pos.y,
        self_pos.pos.z,
        act_index,
    );
    *sub_seq = sub_seq.wrapping_add(1);
    if let Err(e) = map
        .send_encrypted(&payload, datagram_header_id(*sub_seq), server_last_seq)
        .await
    {
        tracing::warn!(error = %e, "mog-house exit MAPRECT send failed");
        let _ = event_tx.send(AgentEvent::Error {
            message: format!("MogHouseExit send: {e}"),
        });
    } else {
        *pending_maprect = Some((std::time::Instant::now(), ZMRQ_LE));
    }
}

async fn send_myroom_job(
    map: &mut MapClient,
    main_job: Option<u8>,
    sub_job: Option<u8>,
    sub_seq: &mut u16,
    server_last_seq: u16,
    event_tx: &broadcast::Sender<AgentEvent>,
) {
    let payload = build_subpacket_myroom_job(*sub_seq, main_job, sub_job);
    tracing::info!(?main_job, ?sub_job, "sending 0x100 MYROOM_JOB");
    *sub_seq = sub_seq.wrapping_add(1);
    if let Err(e) = map
        .send_encrypted(&payload, datagram_header_id(*sub_seq), server_last_seq)
        .await
    {
        tracing::warn!(error = %e, "myroom_job send failed");
        let _ = event_tx.send(AgentEvent::Error {
            message: format!("ChangeJob send: {e}"),
        });
    }
}

// c2s 0x100 GP_CLI_COMMAND_MYROOM_JOB: MainJobIndex u8 @4, SupportJobIndex u8 @5,
// u16 pad; 0 = keep the current job
// (vendor/server/src/map/packets/c2s/0x100_myroom_job.h:27-31).
pub fn build_subpacket_myroom_job(sync: u16, main_job: Option<u8>, sub_job: Option<u8>) -> Vec<u8> {
    let mut buf = vec![0u8; 8];
    buf[0..4].copy_from_slice(&build_subpacket_header(
        ffxi_proto::map::c2s::MYROOM_JOB,
        2,
        sync,
    ));
    buf[4] = main_job.unwrap_or(0);
    buf[5] = sub_job.unwrap_or(0);
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

    buf[4..8].copy_from_slice(&ZMRQ_LE.to_le_bytes());
    buf[8..12].copy_from_slice(&x.to_le_bytes());
    buf[12..16].copy_from_slice(&y.to_le_bytes());
    buf[16..20].copy_from_slice(&z.to_le_bytes());
    buf[20..22].copy_from_slice(&act_index.to_le_bytes());
    buf[22] = exit_bit;
    buf[23] = exit_mode;
    buf
}

/// Client-side mirror of the LSB 0x05D validator: `blockedBy InEvent`,
/// `oneOf<EmoteMode>`, `range Number Point..=Aim` (0x05d_motion.cpp:43-49) and
/// the bell note range (:82). `None` = OK to send. The bell-equip and
/// job-unlock checks stay server-side (the client lacks lockstyle state).
fn emote_send_block_reason(emote_id: u8, mode: u8, param: u16, in_event: bool) -> Option<String> {
    use ffxi_proto::map::emote;
    if in_event {
        return Some("busy with an event".into());
    }
    if mode > emote::mode::MOTION {
        return Some(format!("invalid mode {mode}"));
    }
    if ffxi_proto::emote_names::lookup(emote_id).is_none() {
        return Some(format!("unknown emote id {emote_id}"));
    }
    if emote_id == emote::BELL && !(emote::BELL_NOTE_MIN..=emote::BELL_NOTE_MAX).contains(&param) {
        return Some(format!(
            "bell note {param} out of range {}..={}",
            emote::BELL_NOTE_MIN,
            emote::BELL_NOTE_MAX
        ));
    }
    None
}

// Head-look targid we broadcast: a target-bearing command aimed at ourselves (or
// no target) reads as "looking at nothing", which retail encodes as facetarget 0.
fn face_target_for(target_index: u16, self_act_index: Option<u16>) -> u16 {
    if target_index == 0 || Some(target_index) == self_act_index {
        0
    } else {
        target_index
    }
}

fn build_subpacket_pos(
    sync: u16,
    x: f32,
    y: f32,
    z: f32,
    heading: u8,
    face_target: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 32];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x015, 8, sync));
    buf[4..8].copy_from_slice(&x.to_le_bytes());
    buf[8..12].copy_from_slice(&z.to_le_bytes());
    buf[12..16].copy_from_slice(&y.to_le_bytes());
    buf[20] = heading;
    // GP_CLI_COMMAND_POS.facetarget (vendor/server/.../c2s/0x015_pos.h): the targid
    // we're looking at, relayed by the server so other clients turn our head. +21
    // is the TargetMode/RunMode/GroundMode bitfield, left 0.
    buf[22..24].copy_from_slice(&face_target.to_le_bytes());
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

fn should_release_on_walkaway(user_driven: bool, walk_dist: Option<f32>) -> bool {
    user_driven && walk_dist.is_some_and(|d| d > EVENT_WALKAWAY_YALMS)
}

fn should_break_flood(self_pos_seeded: bool) -> bool {
    self_pos_seeded
}

/// LSB force-places the player at exactly (0,0,0) rot 192 on Mog House zone-in
/// (vendor/server/scripts/globals/moghouse.lua:290), so a MYROOM login's origin
/// seed is authoritative and the near-origin "repair" must not fire.
fn spawn_seed_pos(seed: Vec3, fallback: Option<Vec3>, in_myroom: bool) -> Vec3 {
    if in_myroom {
        return seed;
    }
    apply_zoneline_spawn_fallback(seed, fallback)
}

// XIM synthesizes the MH exit-door actor at native (0, -1, -8) plus a per-model
// doorOffset (research/xim/src/jsMain/kotlin/xim/poc/game/configuration/
// assetviewer/AssetViewer.kt:651-671, model ids per xim/poc/tools/ZoneChanger.kt:
// 18-36); wire entity order swaps the vertical into `z` (GP_SERV_POS_HEAD x/z/y
// "Not a typo", vendor/server/src/map/packets/s2c/0x00a_login.cpp:142-144).
fn mh_door_pos(model: u16) -> Vec3 {
    const MH_2F_MODELS: std::ops::RangeInclusive<u16> = 615..=618;
    const SANDORIA_S: u16 = 745;
    const WINDURST_S: u16 = 219;
    const ADOULIN: u16 = 292;
    const BASTOK_S: u16 = 199;
    let (off_x, off_ground) = match model {
        m if MH_2F_MODELS.contains(&m) => (0.0, -3.15),
        SANDORIA_S => (-0.5, 0.0),
        WINDURST_S | ADOULIN => (-1.0, 0.0),
        BASTOK_S => (-1.15, 0.0),
        _ => (0.0, 0.0),
    };
    Vec3 {
        x: off_x,
        y: -8.0 + off_ground,
        z: -1.0,
    }
}

fn mh_door_entity(model: u16) -> Entity {
    Entity {
        id: crate::local_menu::MH_DOOR_ENTITY_ID,
        act_index: 0,
        kind: EntityKind::Npc,
        name: Some(crate::local_menu::MH_DOOR_NAME.to_string()),
        pos: mh_door_pos(model),
        heading: 0,
        hp_pct: None,
        bt_target_id: 0,
        face_target: 0,
        claim_id: 0,
        speed: 0,
        speed_base: 0,
        look: None,
        npc_state: None,
        status: 0,
    }
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
    fn decode_err_dedup_is_per_opcode() {
        // Opcodes chosen well outside the retail range so parallel tests that
        // exercise real decode paths cannot race on the same entries.
        assert!(first_decode_err(0xFFFE), "first failure must pass the gate");
        assert!(
            !first_decode_err(0xFFFE),
            "repeat failure for the same opcode must be deduped"
        );
        assert!(
            first_decode_err(0xFFFD),
            "dedup must be per-opcode, not global"
        );
    }

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
    fn myroom_login_keeps_forced_origin_seed() {
        // vendor/server/scripts/globals/moghouse.lua:290 setPos(0, 0, 0, 192):
        // the MH origin spawn is authoritative, not a bad seed to repair.
        let town_side = v(162.591, -4.103, 162.423);
        assert_eq!(
            spawn_seed_pos(v(0.0, 0.0, 0.0), Some(town_side), true),
            v(0.0, 0.0, 0.0),
            "MYROOM login must not be desynced to the town-side to_pos"
        );
        assert_eq!(
            spawn_seed_pos(v(0.0, 0.0, 0.0), Some(town_side), false),
            town_side,
            "outside MYROOM the origin repair still applies"
        );
    }

    /// Pins the XIM doorOffset branches (AssetViewer.kt:654-663): 2F interiors
    /// shift the door 3.15 yalms along native z, the [S]-city/Adoulin bases shift
    /// along x.
    #[test]
    fn mh_door_pos_applies_xim_per_model_offsets() {
        assert_eq!(mh_door_pos(257), v(0.0, -8.0, -1.0), "classic 1F");
        assert_eq!(mh_door_pos(745), v(-0.5, -8.0, -1.0), "San d'Oria [S]");
        assert_eq!(mh_door_pos(219), v(-1.0, -8.0, -1.0), "Windurst [S]");
        assert_eq!(mh_door_pos(292), v(-1.0, -8.0, -1.0), "Adoulin");
        assert_eq!(mh_door_pos(199), v(-1.15, -8.0, -1.0), "Bastok [S]");
        for model in 615..=618 {
            let pos = mh_door_pos(model);
            assert_eq!((pos.x, pos.z), (0.0, -1.0), "2F model {model}");
            assert!(
                (pos.y - (-8.0 - 3.15)).abs() < 1e-5,
                "2F model {model} ground offset, got {}",
                pos.y
            );
        }
    }

    #[test]
    fn myroom_job_packet_layout_matches_lsb_struct() {
        let buf = build_subpacket_myroom_job(0xBEEF, Some(5), Some(13));
        assert_eq!(buf.len(), 8, "4 hdr + MainJobIndex + SupportJobIndex + pad");
        let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(id_and_size & 0x1FF, 0x100, "opcode MYROOM_JOB");
        assert_eq!(id_and_size >> 9, 2, "size_words");
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xBEEF, "sync");
        assert_eq!(buf[4], 5, "MainJobIndex");
        assert_eq!(buf[5], 13, "SupportJobIndex");
        assert_eq!(&buf[6..8], &[0, 0], "padding00");

        let keep = build_subpacket_myroom_job(0, None, None);
        assert_eq!(&keep[4..6], &[0, 0], "None → 0 = keep current job");
    }

    /// Pins the c2s 0x05D GP_CLI_COMMAND_MOTION layout
    /// (vendor/server/src/map/packets/c2s/0x05d_motion.h): Mode at byte 11,
    /// BEFORE Param — the s2c 0x05A layout puts Mode after Param, so a
    /// transposition between the two must fail here.
    #[test]
    fn motion_packet_layout_matches_lsb_struct() {
        use ffxi_proto::map::emote;
        let buf = build_subpacket_motion(
            0xBEEF,
            0x0100_0F43,
            0x0443,
            emote::BELL,
            emote::mode::MOTION,
            emote::BELL_NOTE_MIN,
        );
        assert_eq!(
            buf.len(),
            16,
            "hdr + UniqueNo + ActIndex + Number/Mode/Param/pad"
        );
        let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(id_and_size & 0x1FF, ffxi_proto::map::c2s::MOTION, "opcode");
        assert_eq!(id_and_size >> 9, 4, "size_words");
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xBEEF, "sync");
        assert_eq!(
            u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
            0x0100_0F43,
            "UniqueNo"
        );
        assert_eq!(u16::from_le_bytes([buf[8], buf[9]]), 0x0443, "ActIndex");
        assert_eq!(buf[10], emote::BELL, "Number");
        assert_eq!(buf[11], emote::mode::MOTION, "Mode precedes Param (c2s)");
        assert_eq!(
            u16::from_le_bytes([buf[12], buf[13]]),
            emote::BELL_NOTE_MIN,
            "Param"
        );
        assert_eq!(&buf[14..16], &[0, 0], "padding00");
    }

    /// c2s 0x119 GP_CLI_COMMAND_EMOTE_LIST is header-only (4 bytes, 1 word).
    #[test]
    fn emote_list_req_is_header_only() {
        let buf = build_subpacket_emote_list_req(0x1234);
        assert_eq!(buf.len(), 4);
        let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(id_and_size & 0x1FF, ffxi_proto::map::c2s::EMOTE_LIST);
        assert_eq!(id_and_size >> 9, 1, "size_words");
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0x1234, "sync");
    }

    /// Mirrors the LSB 0x05D validator gate order (0x05d_motion.cpp).
    #[test]
    fn emote_send_block_mirrors_lsb_validator() {
        use ffxi_proto::map::emote;
        assert_eq!(emote_send_block_reason(8, emote::mode::ALL, 0, false), None);
        assert!(
            emote_send_block_reason(8, emote::mode::ALL, 0, true).is_some(),
            "blockedBy InEvent"
        );
        assert!(
            emote_send_block_reason(8, 3, 0, false).is_some(),
            "oneOf<EmoteMode>"
        );
        assert!(
            emote_send_block_reason(39, emote::mode::ALL, 0, false).is_some(),
            "39 is a gap in the Emote enum"
        );
        assert!(
            emote_send_block_reason(emote::BELL, emote::mode::ALL, 5, false).is_some(),
            "bell note below 0x06"
        );
        assert_eq!(
            emote_send_block_reason(emote::BELL, emote::mode::ALL, emote::BELL_NOTE_MAX, false),
            None
        );
    }

    #[test]
    fn door_menu_picks_encode_zmrq_maprect_pairs() {
        use crate::local_menu::{Advance, LocalMenuSession, HOME_ROW, MOG_GARDEN_ROW};
        use crate::state::MyRoomInfo;

        let room = MyRoomInfo {
            model: 257,
            sub_map: 0,
            exit_bit: 1,
        };
        for (row, want_bit, want_mode) in [(HOME_ROW, 1u8, 0u8), (MOG_GARDEN_ROW, 0, 127)] {
            let mut menu = LocalMenuSession::new();
            let frame = menu.open_mh_exit(&room, None);
            let idx = frame.choices.iter().position(|c| c == row).expect(row);
            let Advance::Exit(kind) = menu.advance(Some(idx as u32)) else {
                panic!("{row} must be a terminal exit");
            };
            let (bit, mode) = kind.wire_pair();
            let buf = build_subpacket_maprect_mh_exit(7, bit, mode, 1.0, 2.0, 3.0, 0x42);
            assert_eq!(&buf[4..8], b"zmrq", "RectID fourcc");
            assert_eq!(buf[22], want_bit, "MyRoomExitBit for {row}");
            assert_eq!(buf[23], want_mode, "MyRoomExitMode for {row}");
        }
        assert_eq!(u32::from_le_bytes(*b"zmrq"), ZMRQ_LE);
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
    fn walkaway_release_only_user_driven_past_threshold() {
        assert!(!should_release_on_walkaway(
            false,
            Some(EVENT_WALKAWAY_YALMS + 1.0)
        ));
        assert!(!should_release_on_walkaway(true, None));
        assert!(!should_release_on_walkaway(
            true,
            Some(EVENT_WALKAWAY_YALMS - 0.1)
        ));
        assert!(should_release_on_walkaway(
            true,
            Some(EVENT_WALKAWAY_YALMS + 0.1)
        ));
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
        let buf = build_subpacket_event_end(0x1234, 0xDEADBEEF, 0x4242, 230, 535, 0);
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
            &230u16.to_le_bytes(),
            "EventNum carries the zone id (retail echoes LOGIN EventNum, \
             0x00a_login.cpp:187); LSB's 0x05B handler never reads it",
        );
    }

    /// Pins the c2s 0x064 GP_CLI_COMMAND_SCENARIOITEM layout against
    /// vendor/server/src/map/packets/c2s/0x064_scenarioitem.h: UniqueNo u32 @4,
    /// LookItemFlag u32[16] @8, ActIndex u16 @72, TableIndex u16 @74.
    #[test]
    fn scenario_item_packet_layout_matches_lsb_struct() {
        let mut look = [0u32; decode::ScenarioItem::WORDS];
        look[0] = 0b101;
        look[15] = 0x8000_0001;
        let buf = build_subpacket_scenario_item(0x1234, 0xDEAD_BEEF, 0x4242, 6, &look);
        assert_eq!(buf.len(), 76, "header(4) + body(72)");

        let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(id_and_size & 0x1FF, ffxi_proto::map::c2s::SCENARIO_ITEM);
        assert_eq!(id_and_size >> 9, 19, "size_words = 76/4");
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0x1234, "sync");

        assert_eq!(&buf[4..8], &0xDEAD_BEEFu32.to_le_bytes(), "UniqueNo");
        assert_eq!(&buf[8..12], &0b101u32.to_le_bytes(), "LookItemFlag[0]");
        assert_eq!(
            &buf[68..72],
            &0x8000_0001u32.to_le_bytes(),
            "LookItemFlag[15]"
        );
        assert_eq!(&buf[72..74], &0x4242u16.to_le_bytes(), "ActIndex");
        assert_eq!(&buf[74..76], &6u16.to_le_bytes(), "TableIndex");
    }

    /// vendor/server/src/map/packets/c2s/0x064_scenarioitem.cpp:44 —
    /// keyItemId = TableIndex*512 + i*32 + bit; the fold must be its inverse.
    #[test]
    fn mark_seen_bits_round_trip_through_ids_from_flags() {
        let mut flags = [0u32; decode::ScenarioItem::WORDS];
        assert!(fold_seen_ids_into_look_flags(
            1,
            &[513, 512 + 95, 3],
            &mut flags
        ));
        assert_eq!(flags[0], 1 << 1, "id 513 = table 1 bit 1");
        assert_eq!(flags[2], 1 << 31, "id 607 = word 2 bit 31");
        assert_eq!(
            decode::ScenarioItem::ids_from_flags(1, &flags),
            vec![513, 607],
            "round-trips through the tested decode oracle; id 3 (table 0) ignored"
        );
        assert!(
            !fold_seen_ids_into_look_flags(1, &[513], &mut flags),
            "already-set bits report no change"
        );
    }

    /// vendor/server/src/map/packets/c2s/0x064_scenarioitem.cpp:31-33 —
    /// UniqueNo must equal char id and ActIndex must equal targid, ActIndex 0
    /// is always rejected, and the send is blocked while InEvent; every
    /// blocked case must skip without mutating local seen-state.
    #[test]
    fn mark_seen_requires_self_targid() {
        assert!(mark_seen_send_block_reason(false, None, true).is_err());
        assert!(mark_seen_send_block_reason(true, Some(0x123), true).is_err());
        assert!(
            mark_seen_send_block_reason(false, Some(0x123), false).is_err(),
            "table without a received 0x055 must not synthesize an empty update"
        );
        assert_eq!(
            mark_seen_send_block_reason(false, Some(0x123), true),
            Ok(0x123)
        );
    }

    fn tnw(mes_num: u16, num: [i32; 4], name: &str) -> decode::TalkNumWork {
        let mut name_buf = [0u8; decode::TalkNumWork::NAME_LEN];
        name_buf[..name.len()].copy_from_slice(name.as_bytes());
        decode::TalkNumWork {
            unique_no: 0x100,
            num,
            act_index: 5,
            mes_num,
            kind: 0,
            flag: 0,
            name: name_buf,
        }
    }

    #[test]
    fn talknumwork_resolves_key_item_marker_from_zone_text() {
        // Key item 1 = Zeruhn Report (vendor/server/scripts/enum/key_item.lua).
        let line = talknumwork_chat_line(
            &tnw(
                6438 | decode::TalkNumWork::MESNUM_HIDE_NAME_FLAG,
                [1, 0, 0, 0],
                "",
            ),
            Some("Obtained key item: {KeyItem:0}.".to_string()),
            "Zeid",
        );
        assert_eq!(line.text, "Obtained key item: Zeruhn Report.");
        assert_eq!(line.channel, ChatChannel::System);
        assert_eq!(line.sender, "");
    }

    /// Zone-230 KEYITEM_OBTAINED for the client era the default install
    /// carries — LSB text ids are identity DAT entry indexes (LandSandBoat
    /// b3af49c62ae2 IDs.lua pinned 6437 when its sync matched this DAT era;
    /// newer pins say 6438 only because SE inserted entries in later clients —
    /// see ffxi-dat dmsg::tests::real_zone230_keyitem_obtained_decodes_marker).
    const ZONE230_KEYITEM_OBTAINED_MAY2023: u16 = 6437;

    fn test_dat_root() -> Option<ffxi_dat::DatRoot> {
        if let Ok(root) = ffxi_dat::DatRoot::from_env() {
            return Some(root);
        }
        let default = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(ffxi_dat::archive::DEFAULT_INSTALL_DIR);
        ffxi_dat::DatRoot::open(default).ok()
    }

    /// Full 0x02A chat composition against the retail DAT: the zone string's
    /// inline key-item tag must decode to `{KeyItem:0}` and resolve through
    /// `num[0]` (pre-fix this line rendered "Obtained key item: {Auto:128}3\u{FFFD}.").
    /// Key item 1 = Zeruhn Report (vendor/server/scripts/enum/key_item.lua).
    /// Self-skips without game files.
    #[test]
    fn talknumwork_composes_real_keyitem_line_from_zone_dat() {
        let Some(root) = test_dat_root() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let mut ds = crate::event_dialog::DialogSession::new(
            Some(std::sync::Arc::new(root)),
            "Tester".into(),
        );
        let zone_text = ds.zone_text(230, ZONE230_KEYITEM_OBTAINED_MAY2023 as usize);
        assert!(zone_text.is_some(), "zone 230 string DAT must load");
        let line = talknumwork_chat_line(
            &tnw(
                ZONE230_KEYITEM_OBTAINED_MAY2023 | decode::TalkNumWork::MESNUM_HIDE_NAME_FLAG,
                [1, 0, 0, 0],
                "",
            ),
            zone_text,
            "Tester",
        );
        assert_eq!(line.text, "Obtained key item: Zeruhn Report.");
        assert_eq!(line.channel, ChatChannel::System);
    }

    #[test]
    fn talknumwork_shows_speaker_name_when_not_hidden() {
        let line = talknumwork_chat_line(
            &tnw(100, [7, 0, 0, 0], "Trion"),
            Some("{SpeakerName} counts {Num:0}.".to_string()),
            "Zeid",
        );
        assert_eq!(line.sender, "Trion");
        assert_eq!(line.channel, ChatChannel::Say);
        assert_eq!(line.text, "Trion counts 7.");
    }

    #[test]
    fn talknumwork_degrades_to_placeholder_without_zone_strings() {
        let line = talknumwork_chat_line(
            &tnw(
                6438 | decode::TalkNumWork::MESNUM_HIDE_NAME_FLAG,
                [512, 0, 0, 0],
                "",
            ),
            None,
            "Zeid",
        );
        assert!(
            line.text.contains("6438") && line.text.contains("512"),
            "placeholder must expose the masked index and params: {}",
            line.text
        );
    }

    /// A 0x02A buffered during the zone-in flood must replay as a chat line
    /// once the keepalive loop's DialogSession exists — degrading to the
    /// placeholder without a DAT, never dropping silently.
    #[test]
    fn buffered_flood_talknumwork_replays_as_chat_line() {
        let mut ds = crate::event_dialog::DialogSession::new(None, "Tester".into());
        let (tx, mut rx) = broadcast::channel(4);
        let mut body = vec![0u8; decode::TalkNumWork::SIZE];
        body[22..24].copy_from_slice(&42u16.to_le_bytes());
        emit_talknumwork_chat(&body, &mut ds, 230, "Tester", &tx);
        let AgentEvent::ChatLine { line } = rx.try_recv().expect("replay must emit an event")
        else {
            panic!("expected ChatLine");
        };
        assert!(
            line.text.contains("zone message 42"),
            "no-DAT replay degrades to the placeholder: {}",
            line.text
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

    /// Model of LSB's c2s dispatch window
    /// (vendor/server/src/map/map_networking.cpp:419-428,471): subpacket
    /// dispatched iff `client_packet_id < sync <= header`, then
    /// `client_packet_id = header`. Feeds it bundles built the way the
    /// session builds them (one sync per subpacket, header from
    /// [`datagram_header_id`]) and asserts nothing is ever skipped —
    /// multi-subpacket bundles are the case most exposed to a header
    /// counter that drifts from the subpacket syncs, which silently
    /// deafens the server to the session.
    #[test]
    fn datagram_header_keeps_every_subpacket_inside_the_server_window() {
        let mut client_packet_id: u16 = crate::map_client::BOOTSTRAP_SUB_SYNC;
        let mut sub_seq: u16 = crate::map_client::BOOTSTRAP_SUB_SYNC.wrapping_add(1);

        for bundle_len in [1usize, 1, 2, 1, 3, 1, 1, 5, 2, 1] {
            let mut syncs = Vec::new();
            for _ in 0..bundle_len {
                syncs.push(sub_seq);
                sub_seq = sub_seq.wrapping_add(1);
            }
            let header = datagram_header_id(sub_seq);
            assert_eq!(header, *syncs.last().unwrap());
            for sync in syncs {
                assert!(
                    client_packet_id < sync && sync <= header,
                    "sync {sync} outside server window ({client_packet_id}, {header}]"
                );
            }
            client_packet_id = header;
        }
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
    fn shop_sell_req_packet_layout_matches_server_struct() {
        let buf = build_subpacket_shop_sell_req(0xABCD, 7, 4096, 11);
        assert_eq!(buf.len(), 12, "header (4) + body (8)");
        let hdr = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(hdr & 0x01FF, 0x084, "opcode in low 9 bits");
        assert_eq!((hdr >> 9) & 0x7F, 3, "size_words=3");
        assert_eq!(
            u16::from_le_bytes([buf[2], buf[3]]),
            0xABCD,
            "sync echoed in header"
        );
        assert_eq!(
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            7,
            "ItemNum"
        );
        assert_eq!(
            u16::from_le_bytes(buf[8..10].try_into().unwrap()),
            4096,
            "ItemNo"
        );
        assert_eq!(buf[10], 11, "ItemIndex");
        assert_eq!(buf[11], 0, "padding");
    }

    #[test]
    fn shop_sell_set_packet_layout_matches_server_struct() {
        let buf = build_subpacket_shop_sell_set(0xBEEF);
        assert_eq!(buf.len(), 8, "header (4) + body (4)");
        let hdr = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(hdr & 0x01FF, 0x085, "opcode in low 9 bits");
        assert_eq!((hdr >> 9) & 0x7F, 2, "size_words=2");
        assert_eq!(
            u16::from_le_bytes([buf[2], buf[3]]),
            0xBEEF,
            "sync echoed in header"
        );
        assert_eq!(
            u16::from_le_bytes([buf[4], buf[5]]),
            1,
            "SellFlag must be 1 to pass the server validator"
        );
        assert_eq!(&buf[6..8], &[0u8; 2], "padding");
    }

    #[test]
    fn shop_sell_decode_reads_price_slot_count() {
        let mut body = vec![0u8; 12];
        body[0..4].copy_from_slice(&1250u32.to_le_bytes());
        body[4] = 9;
        body[8..12].copy_from_slice(&12u32.to_le_bytes());
        assert_eq!(decode_shop_sell(&body), Some((1250, 9, 12)));
        assert_eq!(decode_shop_sell(&body[..11]), None, "short body rejected");
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
    fn equip_set_packet_layout_matches_server_struct() {
        // GP_CLI_COMMAND_EQUIP_SET (vendor/server/src/map/packets/c2s/0x050_equip_set.h):
        // PropertyItemIndex(u8), EquipKind(u8), Category(u8).
        let buf = build_subpacket_equip_set(0xBEEF, 7, 10, 0);
        assert_eq!(buf.len(), 8, "header (4) + body (4)");
        let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(hdr_word & 0x01FF, 0x050, "opcode = 0x050 EQUIP_SET");
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xBEEF, "sync");
        assert_eq!(buf[4], 7, "PropertyItemIndex = container_index (slotID)");
        assert_eq!(buf[5], 10, "EquipKind = equip_slot (Waist)");
        assert_eq!(buf[6], 0, "Category = container (LOC_INVENTORY)");
    }

    #[test]
    fn item_stack_packet_layout_matches_server_struct() {
        // GP_CLI_COMMAND_ITEM_STACK (vendor/server/src/map/packets/c2s/0x03a_item_stack.h):
        // a single u32 Category (container id) after the 4-byte subpacket header.
        let buf = build_subpacket_item_stack(0xCAFE, 0);
        assert_eq!(buf.len(), 8, "header (4) + Category u32 (4)");
        let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(hdr_word & 0x01FF, 0x03A, "opcode = 0x03A ITEM_STACK");
        assert_eq!((hdr_word >> 9) & 0x7F, 2, "size_words = 2 (8 bytes)");
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xCAFE, "sync");
        assert_eq!(
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            0,
            "Category = container (LOC_INVENTORY = 0)"
        );

        let buf = build_subpacket_item_stack(0, 1);
        assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), 1);
    }

    #[test]
    fn item_move_packet_layout_matches_server_struct() {
        // GP_CLI_COMMAND_ITEM_MOVE (vendor/server/src/map/packets/c2s/0x029_item_move.h):
        // ItemNum u32, Category1 u8, Category2 u8, ItemIndex1 u8, ItemIndex2 u8.
        let buf = build_subpacket_item_move(0xBEEF, 12, 0, 1, 7, None);
        assert_eq!(buf.len(), 12, "header (4) + ItemNum (4) + 4 bytes");
        let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(hdr_word & 0x01FF, 0x029, "opcode = 0x029 ITEM_MOVE");
        assert_eq!((hdr_word >> 9) & 0x7F, 3, "size_words = 3 (12 bytes)");
        assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xBEEF, "sync");
        assert_eq!(
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            12,
            "ItemNum = quantity"
        );
        assert_eq!(buf[8], 0, "Category1 = from LOC_INVENTORY");
        assert_eq!(buf[9], 1, "Category2 = to LOC_MOGSAFE");
        assert_eq!(buf[10], 7, "ItemIndex1 = from slot");
        // The server treats ItemIndex2 < 82 as a stack-merge target; 0xFF asks
        // for a free slot (0x029_item_move.cpp process).
        assert_eq!(buf[11], 0xFF, "ItemIndex2 = auto slot");

        let buf = build_subpacket_item_move(0, 1, 2, 0, 3, Some(9));
        assert_eq!(buf[11], 9, "explicit ItemIndex2 = stack merge slot");
    }

    /// Pins every 0x04D encoding to LSB's PacketValidator rules
    /// (vendor/server/src/map/packets/c2s/0x04d_pbx.cpp validate): a field the
    /// validator mustEquals is hard-coded, unused numerics are -1, and
    /// Result/ResParam1-3 are zero — any drift is a silent server-side drop.
    #[test]
    fn pbx_packet_layout_matches_lsb_validator() {
        use crate::state::{DeliveryBoxNo, DeliveryBoxOp as Op};
        use ffxi_proto::map::pbx::{boxno, command};

        let fields = |op: &Op| {
            let buf = build_subpacket_pbx(0xBEEF, op);
            assert_eq!(buf.len(), 32, "GP_CLI_COMMAND_PBX is 32 bytes");
            let hdr = u16::from_le_bytes(buf[0..2].try_into().unwrap());
            assert_eq!(hdr & 0x01FF, 0x04D, "opcode = 0x04D PBX");
            assert_eq!((hdr >> 9) & 0x7F, 8, "size_words = 8");
            assert_eq!(&buf[12..16], &[0, 0, 0, 0], "Result/ResParam1-3 zero");
            (
                buf[4],
                buf[5] as i8,
                buf[6] as i8,
                buf[7] as i8,
                i32::from_le_bytes(buf[8..12].try_into().unwrap()),
                buf[16..32].to_vec(),
            )
        };

        let no_name = vec![0u8; 16];

        let (cmd, b, pw, iw, st, name) = fields(&Op::PostOpen);
        assert_eq!(
            (cmd, b, pw, iw, st),
            (command::POST_OPEN, boxno::NONE, -1, -1, -1)
        );
        assert_eq!(name, no_name);

        let (cmd, b, pw, iw, st, _) = fields(&Op::Work {
            box_no: DeliveryBoxNo::Incoming,
        });
        assert_eq!(
            (cmd, b, pw, iw, st),
            (command::WORK, boxno::INCOMING, -1, -1, -1)
        );

        let (cmd, b, pw, iw, st, _) = fields(&Op::Check {
            box_no: DeliveryBoxNo::Outgoing,
        });
        assert_eq!(
            (cmd, b, pw, iw, st),
            (command::CHECK, boxno::OUTGOING, -1, -1, -1)
        );

        // Recv: BoxNo pinned Incoming, ItemWorkNo pinned 1.
        let (cmd, b, pw, iw, st, _) = fields(&Op::Recv { slot: 3 });
        assert_eq!(
            (cmd, b, pw, iw, st),
            (command::RECV, boxno::INCOMING, 3, 1, -1)
        );

        let (cmd, b, pw, iw, st, name) = fields(&Op::Set {
            slot: 2,
            inventory_slot: 11,
            quantity: 12,
            recipient: "Atti".into(),
        });
        assert_eq!(
            (cmd, b, pw, iw, st),
            (command::SET, boxno::OUTGOING, 2, 11, 12)
        );
        assert_eq!(&name[..5], b"Atti\0", "NUL-terminated TargetName");

        let (cmd, b, pw, iw, st, _) = fields(&Op::Send { slot: 2 });
        assert_eq!(
            (cmd, b, pw, iw, st),
            (command::SEND, boxno::OUTGOING, 2, -1, -1)
        );

        let (cmd, b, pw, ..) = fields(&Op::Cancel { slot: 4 });
        assert_eq!((cmd, b, pw), (command::CANCEL, boxno::OUTGOING, 4));

        let (cmd, b, pw, ..) = fields(&Op::Accept { slot: 5 });
        assert_eq!((cmd, b, pw), (command::ACCEPT, boxno::INCOMING, 5));

        let (cmd, b, pw, ..) = fields(&Op::Reject { slot: 6 });
        assert_eq!((cmd, b, pw), (command::REJECT, boxno::INCOMING, 6));

        let (cmd, b, pw, ..) = fields(&Op::Get {
            box_no: DeliveryBoxNo::Outgoing,
            slot: 7,
        });
        assert_eq!((cmd, b, pw), (command::GET, boxno::OUTGOING, 7));

        let (cmd, b, pw, ..) = fields(&Op::Clear {
            box_no: DeliveryBoxNo::Incoming,
            slot: 0,
        });
        assert_eq!((cmd, b, pw), (command::CLEAR, boxno::INCOMING, 0));

        let (cmd, b, pw, iw, st, name) = fields(&Op::Query {
            recipient: "Nicotine".into(),
        });
        assert_eq!(
            (cmd, b, pw, iw, st),
            (command::QUERY, boxno::NONE, -1, -1, -1)
        );
        assert_eq!(&name[..9], b"Nicotine\0");

        let (cmd, b, ..) = fields(&Op::Confirm);
        assert_eq!((cmd, b), (command::CONFIRM, boxno::NONE));

        let (cmd, b, ..) = fields(&Op::DeliOpen);
        assert_eq!((cmd, b), (command::DELI_OPEN, boxno::NONE));

        // PostClose: BoxNo pinned None regardless of which box is closing.
        let (cmd, b, ..) = fields(&Op::PostClose {
            box_no: DeliveryBoxNo::Outgoing,
        });
        assert_eq!((cmd, b), (command::POST_CLOSE, boxno::NONE));

        // A 15-char name (FFXI max) still leaves its NUL terminator in place.
        let (.., name) = fields(&Op::Query {
            recipient: "Abcdefghijklmnop".into(),
        });
        assert_eq!(name[15], 0, "TargetName[15] stays NUL");
    }

    #[test]
    fn item_stack_throttle_is_per_container() {
        use std::time::{Duration, Instant};
        let mut last = std::collections::HashMap::new();
        let t0 = Instant::now();
        assert!(item_stack_allowed(&mut last, 0, t0), "first send passes");
        assert!(
            !item_stack_allowed(&mut last, 0, t0 + Duration::from_millis(500)),
            "second within the interval is throttled"
        );
        assert!(
            item_stack_allowed(&mut last, 1, t0 + Duration::from_millis(500)),
            "a different container is independent"
        );
        assert!(
            item_stack_allowed(&mut last, 0, t0 + ITEM_STACK_MIN_INTERVAL),
            "passes again once the interval has elapsed"
        );
    }

    #[test]
    fn item_stack_interval_clears_server_window() {
        // LSB trips at faster than 1/sec; the client margin must stay >= 1s.
        assert!(ITEM_STACK_MIN_INTERVAL >= std::time::Duration::from_secs(1));
    }

    #[test]
    fn equip_set_unequip_uses_zero_slot_index() {
        // LSB unequips a slot when PropertyItemIndex (slotID) is 0, regardless of
        // container: vendor/server/src/map/utils/charutils.cpp:3147
        // ("slotID of zero = unequip"). The re-select-to-unequip path encodes this.
        let buf = build_subpacket_equip_set(0, 0, 10, 0);
        assert_eq!(buf[4], 0, "slotID 0 = unequip");
        assert_eq!(buf[5], 10, "still targets the equip slot being cleared");
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
            (k::EMOTION, ChatChannel::Emote),
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
