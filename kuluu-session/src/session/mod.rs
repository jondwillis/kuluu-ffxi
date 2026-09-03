use anyhow::{anyhow, Context, Result};
use ffxi_proto::{decode, framing};
use tokio::sync::{broadcast, mpsc};

use crate::auth_client::AuthClient;
use crate::event_dialog::EventTrigger;
use crate::lobby_client::LobbyClient;
use crate::map_client::{self, BootstrapArgs, MapClient};
use crate::state::{
    AgentCommand, AgentEvent, BlowfishStatus, ChatChannel, ChatLine, Diagnostics, Entity,
    EntityKind, HealMode, InventoryUpdate, ItemSlot, Position, ShopItem, ShopState, Stage, Vec3,
};

mod codec;
mod treasure;

pub(crate) use codec::*;
pub use codec::{
    build_subpacket_action, build_subpacket_auc_ask_commit, build_subpacket_auc_bid,
    build_subpacket_auc_info, build_subpacket_auc_lot_cancel, build_subpacket_auc_lot_check,
    build_subpacket_auc_lot_in, build_subpacket_auc_work_check, build_subpacket_bazaar_buy,
    build_subpacket_bazaar_exit, build_subpacket_bazaar_list, build_subpacket_buffcancel,
    build_subpacket_camp, build_subpacket_emote_list_req, build_subpacket_equip_inspect,
    build_subpacket_equip_set, build_subpacket_fishing, build_subpacket_item_move,
    build_subpacket_item_stack, build_subpacket_item_use, build_subpacket_motion,
    build_subpacket_myroom_job, build_subpacket_pbx, build_subpacket_reqlogout,
    build_subpacket_shop_buy, build_subpacket_shop_sell_req, build_subpacket_shop_sell_set,
    build_subpacket_tracking_end, build_subpacket_tracking_list, build_subpacket_tracking_start,
};

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
                        .handshake(&auth_session, *cid, 0, key3)
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

    // The spell DAT (ROM/118/114.DAT) is zone-invariant: load it once off the
    // runtime instead of re-reading it on every zone change inside the loop.
    let spell_table: Option<std::sync::Arc<ffxi_dat::spell_info::SpellTable>> =
        match cfg.dat_root.clone() {
            Some(root) => tokio::task::spawn_blocking(move || {
                ffxi_dat::spell_info::SpellTable::open(root.root())
            })
            .await
            .ok()
            .map(std::sync::Arc::new),
            None => None,
        };

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
            spell_table.clone(),
        )
        .await?;

        match outcome {
            MapOutcome::Disconnected => return Ok(()),
            MapOutcome::Reconnect {
                new_addr,
                via_zoneline,
            } => {
                spawn_fallback = via_zoneline
                    .and_then(kuluu_nav::to_pos_for_line)
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
    spell_table: Option<std::sync::Arc<ffxi_dat::spell_info::SpellTable>>,
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

    // Safety cap for the pre-GAMEOK drain (the self-seed normally lands well
    // before it). The post-send quiescence drains below are short by design:
    // each only needs to observe the server's response to the previous send so
    // the next c2s carries a fresh ack — parse() drops any non-LOGIN c2s whose
    // ack != server_packet_id (vendor/server/src/map/map_networking.cpp:478),
    // and that counter advances once per processed c2s.
    const FLOOD_DRAIN_CAP: std::time::Duration = std::time::Duration::from_secs(8);
    let flood_deadline = std::time::Instant::now() + FLOOD_DRAIN_CAP;
    let mut server_last_seq: u16 = 0;
    let mut total_subs = 0usize;
    let mut pending_event_end: Vec<(u32, u16, u16)> = Vec::new();
    // Scoped owner of everything an event script's cues change; see
    // `CutsceneScope`. Declared with the session so every exit path out of it
    // — including the bootstrap flood — releases through the same object.
    let mut cutscene = crate::event_dialog::CutsceneScope::default();
    let mut self_act_index: Option<u16> = None;
    let mut name_cache: std::collections::HashMap<u32, String> = Default::default();

    let mut kind_cache: std::collections::HashMap<u32, crate::state::EntityKind> =
        Default::default();

    let mut claim_cache: std::collections::HashMap<u32, u32> = Default::default();

    let mut npc_name_resolver = NpcNameResolver::new(cfg.dat_root.clone());
    let mut emote_text_resolver = EmoteTextResolver::new(cfg.dat_root.clone());
    let mut sysmes_resolver = treasure::SysMesResolver::new(cfg.dat_root.clone());
    let mut treasure_pool = treasure::TreasurePool::default();

    let mut name_miss_dedup: std::collections::HashMap<
        (u32, crate::state::NameMissKind),
        std::time::Instant,
    > = Default::default();
    let mut current_zone_id: u16 = 0;

    let mut self_pos = Position::default();

    let mut self_pos_seeded = false;

    let mut flood_in_mog_house = false;

    let mut mog = SelfMogState::default();

    let mut flood_zone_messages: Vec<(u16, Vec<u8>)> = Vec::new();
    drain_zone_flood(
        map,
        flood_deadline,
        false,
        &mut server_last_seq,
        &mut total_subs,
        &mut self_pos_seeded,
        event_tx,
        &mut pending_event_end,
        &mut cutscene,
        bootstrap.char_id,
        bootstrap.char_name,
        &mut self_act_index,
        &mut name_cache,
        &mut kind_cache,
        &mut claim_cache,
        &mut name_miss_dedup,
        &mut current_zone_id,
        &mut self_pos,
        &mut npc_name_resolver,
        &mut emote_text_resolver,
        &mut sysmes_resolver,
        &mut treasure_pool,
        &mut flood_in_mog_house,
        &mut mog,
        spawn_fallback,
        &mut flood_zone_messages,
    )
    .await;
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

    // Each c2s below is followed by a quiescence drain of the server's reply:
    // parse() advances server_packet_id once per processed c2s and stamps that
    // value on its response, so the next send must carry the ack observed in
    // that reply — firing back-to-back with one captured ack gets every send
    // after the first eaten by the retransmit guard above.
    macro_rules! quiesce {
        () => {
            drain_zone_flood(
                map,
                std::time::Instant::now() + FLOOD_DRAIN_CAP,
                true,
                &mut server_last_seq,
                &mut total_subs,
                &mut self_pos_seeded,
                event_tx,
                &mut pending_event_end,
                &mut cutscene,
                bootstrap.char_id,
                bootstrap.char_name,
                &mut self_act_index,
                &mut name_cache,
                &mut kind_cache,
                &mut claim_cache,
                &mut name_miss_dedup,
                &mut current_zone_id,
                &mut self_pos,
                &mut npc_name_resolver,
                &mut emote_text_resolver,
                &mut sysmes_resolver,
                &mut treasure_pool,
                &mut flood_in_mog_house,
                &mut mog,
                spawn_fallback,
                &mut flood_zone_messages,
            )
            .await;
        };
    }

    {
        let payload = build_subpacket_gameok(sub_seq);
        sub_seq = sub_seq.wrapping_add(1);
        map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
            .await?;
        // `ack` must equal the server's current server_packet_id or parse()
        // drops this c2s and re-sends its cached last s2c instead.
        tracing::info!(
            sub_seq,
            ack = server_last_seq,
            "sent 0x00C GAMEOK (zone-in)"
        );
    }
    quiesce!();
    // c2s 0x076 GROUP_LIST_REQ: request the full party table. The server
    // answers with GROUP_TBL (0x0C8) + GROUP_LIST (0x0DD) for every member.
    // Sent BEFORE 0x061 so the roster (GROUP_TBL) lands ahead of self's
    // GROUP_ATTR, matching the order LSB's own ReloadParty() uses. For a solo
    // player LSB answers with an empty GROUP_TBL(nullptr); the state merge
    // keeps self through that (see PartyTableReset).
    {
        let payload = build_subpacket_group_list_req(sub_seq);
        sub_seq = sub_seq.wrapping_add(1);
        map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
            .await?;
        tracing::info!(
            sub_seq,
            ack = server_last_seq,
            "sent 0x076 GROUP_LIST_REQ (zone-in party request)"
        );
    }
    quiesce!();
    // c2s 0x061 CLISTATUS: request the local player's status block. The server
    // answers with SendLocalPlayerPackets — including s2c GROUP_ATTR (0x0DF) for
    // self, which is the ONLY source of group data on zone-in for a solo player
    // (LSB pushes no 0x0DD/0x0DF to players without a party). Without this the
    // party frame would sit on its default 0/0 draw until the next zone.
    {
        let payload = build_subpacket_clistatus(sub_seq);
        sub_seq = sub_seq.wrapping_add(1);
        map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
            .await?;
        tracing::info!(
            sub_seq,
            ack = server_last_seq,
            "sent 0x061 CLISTATUS (zone-in self status request)"
        );
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
        cutscene,
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
        sysmes_resolver,
        treasure_pool,
        mog,
        flood_zone_messages,
        spell_table,
        cfg.server.clone(),
    )
    .await
}

/// The TALKNUM family resolves against the zone dialog DAT owned by the
/// keepalive loop's DialogSession, so bodies arriving during the flood are
/// buffered and replayed once it exists — never dropped silently. This cap
/// bounds memory against a misbehaving server; zone onZoneIn lua emits only
/// a handful of messageSpecial lines (e.g.
/// vendor/server/scripts/zones/Attohwa_Chasm/Zone.lua).
const FLOOD_ZONE_MESSAGE_MAX: usize = 32;

/// Whether the zone-in flood drain should stop on an idle recv window. Unconditional
/// when `break_on_idle` (the short post-send quiescence drains), otherwise only once
/// the self position seed has landed — so the pre-GAMEOK drain keeps reading until it
/// holds our authoritative spawn before letting the next c2s fire.
fn should_break_flood(break_on_idle: bool, self_pos_seeded: bool) -> bool {
    break_on_idle || self_pos_seeded
}

/// Drains and processes zone-in traffic until `deadline`, or earlier once the
/// socket has been idle for one recv window: unconditionally when
/// `break_on_idle`, otherwise only after the self position seed has landed.
/// Every received datagram refreshes `*server_last_seq` so the next c2s can
/// carry a fresh ack (see parse()'s retransmit guard).
#[allow(clippy::too_many_arguments)]
async fn drain_zone_flood(
    map: &MapClient,
    deadline: std::time::Instant,
    break_on_idle: bool,
    server_last_seq: &mut u16,
    total_subs: &mut usize,
    self_pos_seeded: &mut bool,
    event_tx: &broadcast::Sender<AgentEvent>,
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    cutscene: &mut crate::event_dialog::CutsceneScope,
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
    npc_name_resolver: &mut NpcNameResolver,
    emote_text: &mut EmoteTextResolver,
    sysmes: &mut treasure::SysMesResolver,
    pool: &mut treasure::TreasurePool,
    was_in_mog_house: &mut bool,
    mog: &mut SelfMogState,
    zoneline_spawn_fallback: Option<Vec3>,
    flood_zone_messages: &mut Vec<(u16, Vec<u8>)>,
) {
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(500), map.recv_decrypted())
            .await
        {
            Ok(Ok(buf)) => {
                let header = framing::Header::read(&buf[..framing::FFXI_HEADER_SIZE]);
                *server_last_seq = header.id_and_size;
                for sub in framing::walk_sub_packets(&buf[framing::FFXI_HEADER_SIZE..]).flatten() {
                    *total_subs += 1;
                    if ZONE_MESSAGE_OPCODES.contains(&sub.opcode) {
                        if flood_zone_messages.len() < FLOOD_ZONE_MESSAGE_MAX {
                            flood_zone_messages.push((sub.opcode, sub.data.to_vec()));
                        } else {
                            tracing::warn!(
                                opcode = format!("{:#05X}", sub.opcode),
                                "zone-message flood buffer full; dropping zone message"
                            );
                        }
                        continue;
                    }
                    handle_sub_packet(
                        &sub,
                        event_tx,
                        pending_event_end,
                        cutscene,
                        self_char_id,
                        self_char_name,
                        self_act_index,
                        name_cache,
                        kind_cache,
                        claim_cache,
                        name_miss_dedup,
                        current_zone_id,
                        self_pos,
                        self_pos_seeded,
                        npc_name_resolver,
                        emote_text,
                        sysmes,
                        pool,
                        was_in_mog_house,
                        mog,
                        zoneline_spawn_fallback,
                    );
                }
            }

            Ok(Err(_)) => break,

            Err(_) => {
                if should_break_flood(break_on_idle, *self_pos_seeded) {
                    break;
                }
            }
        }
    }
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
    cutscene: &mut crate::event_dialog::CutsceneScope,
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

    sysmes: &mut treasure::SysMesResolver,

    pool: &mut treasure::TreasurePool,

    was_in_mog_house: &mut bool,

    mog: &mut SelfMogState,

    zoneline_spawn_fallback: Option<Vec3>,
) {
    use ffxi_proto::map::s2c;
    match sub.opcode {
        s2c::LOGIN => {
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

                // The pool is per-zone: the server replays it after zone-in, so
                // the slot -> item mapping 0x0D3 depends on has to start empty
                // or a stale slot would name the wrong item.
                pool.clear();

                let _ = event_tx.send(AgentEvent::ZoneChanged {
                    from: None,
                    to: login.zone_no,
                    myroom: mog.myroom,
                    mog_zone_flag: mog.mog_zone_flag,
                });

                // After ZoneChanged, which clears it: the renderer's sub-area
                // latch seeds from this so a character who logged out inside a
                // shop comes back inside the interior, not inside its shell.
                let _ = event_tx.send(AgentEvent::SubAreaSynced {
                    sub_area: login.sub_area,
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

                // 0x057 WEATHER is only broadcast on a weather *change*
                // (vendor/server/src/map/zone.cpp:672 is its sole construction
                // site), so without this a zoning character renders the default
                // sky until the next change lands. Emitted after ZoneChanged,
                // which clears current_weather.
                if let Some(w) = login.weather {
                    tracing::info!(
                        weather_number = w.weather_number,
                        weather_time = w.weather_time,
                        previous_weather_number = w.previous_weather_number,
                        has_previous = w.has_previous(),
                        offset_time = w.offset_time,
                        "0x00A LOGIN carries zone-in weather"
                    );
                    let _ = event_tx.send(AgentEvent::WeatherUpdated {
                        weather_number: w.weather_number,
                    });
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
                            name_vis: (head.flags3 >> 24) as u8,
                            claim_id: 0,
                            speed: head.speed,
                            speed_base: head.speed_base,

                            look: login.look,
                            npc_state: None,
                            char_flags: None,
                            status: 0,
                            mount_id: None,
                        },

                        pos_present: true,
                    });

                    let _ = event_tx.send(AgentEvent::PositionChanged { pos: *self_pos });
                }
            }
        }
        op @ (s2c::CHAR_PC | s2c::CHAR_NPC) => {
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

                // Retail resolves static NPC names from the zone's NPC-name
                // DAT by id and IGNORES the wire name field for them -- on the
                // wire that field carries the door FourCC for doors ("_6i3")
                // and the internal script name for helpers ("blank"). DAT
                // first; wire only as fallback for ids the DAT does not cover.
                let name = if op == s2c::CHAR_NPC {
                    npc_name_resolver
                        .lookup(head.unique_no)
                        .map(str::to_string)
                        .or(wire_name)
                } else {
                    wire_name
                };

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

                let char_flags =
                    (send_flag & UPDATE_HP != 0).then(|| decode::CharFlags::from_pos_head(&head));

                let status = match op {
                    s2c::CHAR_NPC => decode::NpcState::decode_char_npc_status(sub.data),
                    _ => Some(0),
                }
                .unwrap_or(0);

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
                        name_vis: (head.flags3 >> 24) as u8,
                        claim_id,
                        speed: head.speed,
                        speed_base: head.speed_base,
                        look,
                        npc_state,
                        char_flags,
                        status,
                        // Same General-block gate as npc_state: Flags6 only rides
                        // an update that carries the block, and the field is stale
                        // once it does not.
                        mount_id: (send_flag & UPDATE_HP != 0)
                            .then(|| decode::PosHead::mount_index(sub.data))
                            .flatten(),
                    },
                    pos_present,
                });
            }
        }
        s2c::ENTITY_UPDATE1 => match sub.data.first().copied() {
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
        s2c::ENTITY_UPDATE2 => {
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
        s2c::BATTLE_MESSAGE => {
            if let Some(line) = decode_battle_message(sub.data, name_cache, kind_cache, true) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
            emit_battle_message_audio_event(sub.data, true, event_tx);
        }
        s2c::BATTLE_MESSAGE2 => {
            if let Some(line) = decode_battle_message(sub.data, name_cache, kind_cache, false) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
            emit_battle_message_audio_event(sub.data, false, event_tx);
        }
        s2c::SHOP_LIST => {
            if let Some(shop) = decode_shop_list(sub.data) {
                let _ = event_tx.send(AgentEvent::ShopUpdated { shop });
            }
        }
        s2c::SHOP_SELL => {
            if let Some((price, item_index, count)) = decode_shop_sell(sub.data) {
                let _ = event_tx.send(AgentEvent::ShopSellAppraisal {
                    price,
                    item_index,
                    count,
                });
            }
        }
        s2c::SHOP_OPEN => {}
        s2c::BATTLE2 => {
            if let Some(h) = decode_battle2_header(sub.data) {
                let _ = event_tx.send(AgentEvent::ActionStarted {
                    actor_id: h.actor_id,
                    action_id: h.action_id,
                    action_kind: h.action_kind,
                    target_id: h.primary_target_id,
                    result: h.first_result,
                    animation: h.animation,
                });
            }
            for line in decode_battle2_action(sub.data, name_cache, kind_cache) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
        }
        s2c::MOTIONMES => {
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
        s2c::EMOTE_LIST => {
            if let Ok(e) =
                decode::EmoteList::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::EmoteListUpdated {
                    job_bits: e.job_bits,
                    chair_bits: e.chair_bits,
                });
            }
        }
        s2c::MUSIC => {
            if sub.data.len() >= 4 {
                let slot = u16::from_le_bytes([sub.data[0], sub.data[1]]) as u8;
                let track_id = u16::from_le_bytes([sub.data[2], sub.data[3]]);
                tracing::info!(slot, track_id, "0x05F MUSIC packet");
                let _ = event_tx.send(AgentEvent::MusicChanged { slot, track_id });
            }
        }
        s2c::MUSIC_VOLUME => {
            if sub.data.len() >= 4 {
                let slot = u16::from_le_bytes([sub.data[0], sub.data[1]]) as u8;
                let volume = u16::from_le_bytes([sub.data[2], sub.data[3]]) as u8;
                tracing::info!(slot, volume, "0x060 MUSIC_VOLUME packet");
                let _ = event_tx.send(AgentEvent::MusicVolumeChanged { slot, volume });
            }
        }
        s2c::CHAR_STATUS => {
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
                    let _ = event_tx.send(AgentEvent::SelfServerStatus {
                        status: cs.server_status,
                        mount_id: cs.mount_id,
                    });
                }
            }
        }
        s2c::FISH => {
            if let Ok(f) =
                decode::FishPacket::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::FishHooked { params: f.into() });
            }
        }
        s2c::JOB_INFO => {
            if let Ok(ji) =
                decode::JobInfo::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let info = crate::state::JobInfoState::from(ji);
                mog.job_info = Some(info);
                let _ = event_tx.send(AgentEvent::JobInfoUpdated { info });
            }
        }
        s2c::CLISTATUS => {
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
        s2c::EVENTUCOFF => {
            handle_eventucoff(sub.data, pending_event_end, cutscene, event_tx);
        }
        s2c::WPOS | s2c::WPOS2 => {
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
        s2c::WEATHER => {
            if let Ok(w) = decode::WeatherPacket::decode(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::WeatherUpdated {
                    weather_number: w.weather_number,
                });
            }
        }
        s2c::MISCDATA => {
            if let Some((icons, expiries)) = decode_miscdata_status_icons(sub.data) {
                let _ = event_tx.send(AgentEvent::StatusIconsUpdated { icons, expiries });
            }
        }
        // Treasure pool: 0x0D2 places an item (and/or gil) in a slot, 0x0D3
        // reports a lot, a win, or a loss against one.
        s2c::TROPHY_LIST => treasure::handle_trophy_list(
            sub.data,
            event_tx,
            sysmes,
            pool,
            name_cache,
            self_char_name,
        ),
        s2c::TROPHY_SOLUTION => treasure::handle_trophy_solution(sub.data, event_tx, sysmes, pool),
        s2c::ABIL_RECAST => {
            let recasts = decode_abil_recast(sub.data);
            let _ = event_tx.send(AgentEvent::AbilityRecastsUpdated { recasts });
        }
        // Wide-scan (tracking): the server frames a list with 0x0F6 ListStart, a
        // run of 0x0F4 entries, then 0x0F6 ListEnd; 0x0F5 streams the tracked
        // entity's position (vendor/server/src/map/packets/s2c/0x0f4..0x0f6*).
        s2c::TRACKING_STATE => {
            if let Ok(st) = decode::WidescanState::decode(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                if st.list_start {
                    let _ = event_tx.send(AgentEvent::WidescanListStart);
                }
                if st.list_end {
                    let _ = event_tx.send(AgentEvent::WidescanListEnd);
                }
            }
        }
        s2c::TRACKING_LIST => {
            if let Ok(e) = decode::WidescanEntry::decode(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let mut entry = crate::state::WidescanEntry::from(e);
                // Retail resolves npc/mob wide-scan names client-side from the
                // ActIndex; sName only overrides (research/XiPackets
                // world/server/0x00F4). Servers send sName empty, so name the
                // entry from the zone's NPC-name DAT.
                if entry.name.is_empty() && entry.kind != ffxi_proto::map::tracking::kind::CHAR {
                    if let Some(name) = npc_name_resolver
                        .lookup(ffxi_dat::compose_id(*current_zone_id, entry.act_index))
                    {
                        entry.name = name.to_string();
                    }
                }
                let _ = event_tx.send(AgentEvent::WidescanEntryReceived { entry });
            }
        }
        s2c::TRACKING_POS => {
            if let Ok(p) = decode::WidescanPos::decode(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::WidescanTrackUpdated {
                    tracked: (!p.lost).then(|| crate::state::WidescanPos::from(p)),
                });
            }
        }
        s2c::SCENARIO_ITEM => {
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
        s2c::EVENT => {
            if let Some(dialog) = decode_event_0x032(sub.data) {
                emit_event_dialog(event_tx, &dialog, pending_event_end, name_cache);
            }
        }
        s2c::EVENTSTR => {
            if let Some(dialog) = decode_event_0x033(sub.data) {
                emit_event_dialog(event_tx, &dialog, pending_event_end, name_cache);
            }
        }
        s2c::EVENTNUM => {
            if let Some(dialog) = decode_event_0x034(sub.data) {
                emit_event_dialog(event_tx, &dialog, pending_event_end, name_cache);
            }
        }
        s2c::MESSAGE => {
            if let Some(line) = decode_std_message_examine(sub.data, name_cache) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            } else if let Ok(text) =
                std::str::from_utf8(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let line = ChatLine {
                    spans: Vec::new(),
                    channel: ChatChannel::System,
                    sender: "<server>".into(),
                    text: text.trim_end_matches('\0').to_string(),
                    server_ts: 0,
                };
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
        }
        s2c::EQUIP_INSPECT => match decode::EquipInspect::decode(sub.data) {
            Ok(decode::EquipInspect::Equipment(eq)) => {
                let _ = event_tx.send(AgentEvent::CheckEquipReceived {
                    target_id: eq.unique_no,
                    act_index: eq.act_index,
                    items: eq.items.iter().map(|i| (i.equip_kind, i.item_no)).collect(),
                });
            }
            Ok(decode::EquipInspect::General(g)) => {
                let _ = event_tx.send(AgentEvent::CheckGeneralReceived {
                    target_id: g.unique_no,
                    act_index: g.act_index,
                    main_job: g.main_job,
                    sub_job: g.sub_job,
                    main_job_lv: g.main_job_lv,
                    sub_job_lv: g.sub_job_lv,
                    master_lv: g.master_lv,
                    linkshell: g.linkshell_name(),
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, body_len = sub.data.len(), "0x0C9 EQUIP_INSPECT decode failed");
            }
        },
        s2c::INSPECT_MESSAGE => match decode::InspectMessage::decode(sub.data) {
            Ok(m) => {
                let _ = event_tx.send(AgentEvent::CheckMessageReceived {
                    name: m.name,
                    message: m.message,
                });
            }
            Err(e) => warn_decode_err(sub.opcode, &e),
        },
        s2c::BAZAAR_LIST => match decode::BazaarListItem::decode(sub.data) {
            Ok(row) => {
                let _ = event_tx.send(AgentEvent::BazaarItemReceived {
                    index: row.index,
                    item_no: row.item_no,
                    quantity: row.quantity,
                    price: row.price,
                    tax_rate: row.tax_rate,
                });
            }
            Err(e) => warn_decode_err(sub.opcode, &e),
        },
        s2c::BAZAAR_BUY => match decode::BazaarBuy::decode(sub.data) {
            Ok(buy) => {
                let _ = event_tx.send(AgentEvent::BazaarBuyResult {
                    ok: buy.state == decode::BazaarBuyState::Ok,
                });
            }
            Err(e) => warn_decode_err(sub.opcode, &e),
        },
        s2c::BAZAAR_CLOSE => match decode::BazaarClose::decode(sub.data) {
            Ok(_) => {
                let _ = event_tx.send(AgentEvent::BazaarClosed);
            }
            Err(e) => warn_decode_err(sub.opcode, &e),
        },
        s2c::BAZAAR_SELL => match decode::BazaarSell::decode(sub.data) {
            Ok(sell) => {
                let _ = event_tx.send(AgentEvent::BazaarSoldToOther {
                    buyer: sell.buyer,
                    index: sell.index,
                    quantity: sell.quantity,
                });
            }
            Err(e) => warn_decode_err(sub.opcode, &e),
        },
        s2c::CHAT => {
            if let Some((title, options)) = decode_custom_menu(sub.data) {
                // Retail renders a GMPROMPT/_CUSTOM_MENU as an interactive prompt,
                // not a chat line (the packet's speaker is the player entity).
                let dialog = crate::state::DialogState {
                    // A customMenu carries no speaker entity, so blank the header
                    // (otherwise npc_id 0 renders as `#00000000`).
                    npc_name: Some(String::new()),
                    prompt: Some(title),
                    choices: options,
                    custom_menu: true,
                    ..Default::default()
                };
                let _ = event_tx.send(AgentEvent::EventDialog { dialog });
            } else if let Some(line) = decode_chat_std(sub.data) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
        }
        s2c::SYSTEMMES => {
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
        s2c::GROUP_TBL => {
            if let Ok(tbl) =
                decode::GroupTbl::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                tracing::info!(
                    kind = ?tbl.kind,
                    member_count = tbl.members.len(),
                    "0x0C8 GROUP_TBL (party definition)",
                );
                let _ = event_tx.send(AgentEvent::PartyTableReset {
                    members: tbl.members,
                });
            }
        }
        s2c::GROUP_LIST => {
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
        s2c::GROUP_ATTR => {
            if let Ok(attrs) = decode::PartyAttrs::decode_group_attr(sub.data)
                .inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                tracing::info!(
                    id = attrs.unique_no,
                    is_self = attrs.unique_no == self_char_id,
                    hp = attrs.hp,
                    hpp = attrs.hpp,
                    zone_no = attrs.zone_no,
                    "0x0DF GROUP_ATTR",
                );
                if attrs.unique_no == self_char_id {
                    note_mog_transition(attrs.moghouse_flg != 0, was_in_mog_house, event_tx);
                }
                let _ = event_tx.send(AgentEvent::PartyMemberUpdated {
                    member: party_member_from_attrs(&attrs, None),
                });
            }
        }
        s2c::ITEM_MAX => {
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
                        spans: Vec::new(),
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
        s2c::ITEM_SAME => {
            if let Ok(s) =
                decode::ItemSame::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                if matches!(s.state, decode::ItemSameState::AllLoaded) {
                    let _ = event_tx.send(AgentEvent::InventoryReady);
                }
            }
        }
        s2c::ITEM_NUM => {
            if let Ok(n) =
                decode::ItemNum::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::ChatLine {
                    line: ChatLine {
                        spans: Vec::new(),
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
        s2c::ITEM_LIST => {
            if let Ok(l) =
                decode::ItemList::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::ChatLine {
                    line: ChatLine {
                        spans: Vec::new(),
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
                            charges_remaining: None,
                            next_use_vana_ts: None,
                        },
                    },
                });
            }
        }
        s2c::ITEM_ATTR => {
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
                        spans: Vec::new(),
                        channel: ChatChannel::Debug,
                        sender: "client".into(),
                        text: format!(
                            "📦 Attr: cat={} slot={} item=#{} qty={}{}",
                            a.category, a.index, a.item_no, a.quantity, price_tag,
                        ),
                        server_ts: 0,
                    },
                });
                let ci = a.charge_info();
                let _ = event_tx.send(AgentEvent::InventoryUpdated {
                    container: a.category,
                    update: InventoryUpdate::SlotChanged {
                        slot: ItemSlot {
                            index: a.index,
                            item_no: a.item_no,
                            quantity: a.quantity,
                            locked: a.lock_flg != 0,
                            price: a.price,
                            charges_remaining: ci.map(|c| c.charges),
                            next_use_vana_ts: ci.map(|c| c.next_use_vana_ts),
                        },
                    },
                });
            }
        }
        s2c::EQUIP_CLEAR => {
            let _ = event_tx.send(AgentEvent::EquipCleared);
        }
        s2c::EQUIP_LIST => {
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
        // Self's appearance channel: LSB skips the owning char in every
        // ENTITY_UPDATE/SPAWN broadcast (vendor/server/src/map/zone_entities.cpp
        // `CZoneEntities::UpdateEntityPacket`), so a player never receives a
        // 0x00D CHAR_PC about itself. 0x051 is what it gets instead — at zone-in
        // (packets/c2s/0x00a_login.cpp:131), on gameok (c2s/0x00c_gameok.cpp:67)
        // and on every equip/lockstyle/head-toggle change
        // (entities/charentity.cpp:1174, c2s/0x053_lockstyle.cpp:37,
        // c2s/0x0dc_config.cpp:113).
        s2c::GRAP_LIST => match decode::LookData::decode_grap_list(sub.data) {
            Some(look) => {
                let _ = event_tx.send(AgentEvent::SelfLookUpdated { look });
            }
            // Without this, a wrong body-offset assumption is invisible: self
            // just keeps the launcher seed look with nothing in the log.
            None => warn_decode_err(
                sub.opcode,
                format_args!(
                    "GrapIDTbl at offset {} absent or zeroed in a {}-byte body",
                    decode::LookData::GRAP_LIST_TBL_OFFSET,
                    sub.data.len()
                ),
            ),
        },
        s2c::MAGIC_DATA => {
            if let Ok(m) =
                decode::MagicData::decode(sub.data).inspect_err(|e| warn_decode_err(sub.opcode, e))
            {
                let _ = event_tx.send(AgentEvent::SpellsKnownUpdated { ids: m.known_ids() });
            }
        }
        s2c::COMMAND_DATA => {
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
fn begin_server_event(
    dialog_session: &mut crate::event_dialog::DialogSession,
    trigger: EventTrigger,
    event_tx: &broadcast::Sender<AgentEvent>,
    cutscene: &mut crate::event_dialog::CutsceneScope,
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    auto_event_end: &mut Vec<(u32, u16, u16, u32)>,
) {
    let (zone_id, unique_no, act_index, event_id) = (
        trigger.event_zone,
        trigger.unique_no,
        trigger.act_index,
        trigger.event_id,
    );
    let outcome = dialog_session.begin(trigger);
    let cues = dialog_session.take_cues();
    // A choreography-only script runs to completion inside `begin`, so its
    // cues have to open and close a session of their own or nothing downstream
    // would ever see them.
    if !cues.is_empty() {
        cutscene.start(
            crate::event_dialog::agent_event_id(unique_no, event_id),
            event_tx,
        );
        for cue in cues {
            cutscene.push(cue, event_tx);
        }
    }
    match outcome {
        crate::event_dialog::Begin::Frame(dialog) => {
            cutscene.start(dialog.event_id, event_tx);
            let _ = event_tx.send(AgentEvent::EventStart {
                event_id: dialog.event_id,
            });
            emit_event_speech_to_chat(event_tx, &dialog);
            let _ = event_tx.send(AgentEvent::EventDialog { dialog });
            pending_event_end.push((unique_no, act_index, event_id));
        }
        crate::event_dialog::Begin::Ended { end_para } => {
            cutscene.end(crate::event_dialog::EventSessionExit::ScriptEnded, event_tx);
            auto_event_end.push((unique_no, act_index, event_id, end_para));
        }
        crate::event_dialog::Begin::Waiting => {
            let id = crate::event_dialog::agent_event_id(unique_no, event_id);
            cutscene.start(id, event_tx);
            let _ = event_tx.send(AgentEvent::EventStart { event_id: id });
            pending_event_end.push((unique_no, act_index, event_id));
        }
        crate::event_dialog::Begin::Undriveable { stopped_op, reason } => {
            tracing::warn!(
                zone = zone_id,
                unique_no = format!("0x{unique_no:08X}"),
                act_index,
                event_id,
                reason = reason.as_str(),
                stopped_op = ?stopped_op.map(|op| format!("0x{op:02X}")),
                "auto-releasing VM-undriveable event"
            );
            cutscene.end(crate::event_dialog::EventSessionExit::Cancelled, event_tx);
            auto_event_end.push((unique_no, act_index, event_id, 0));
            let _ = event_tx.send(AgentEvent::ChatLine {
                line: ChatLine {
                    spans: Vec::new(),
                    channel: ChatChannel::System,
                    sender: "client".into(),
                    text: match stopped_op {
                        Some(op) => format!(
                            "[event] cutscene {event_id} auto-skipped (unimplemented opcode 0x{op:02X})"
                        ),
                        None => format!(
                            "[event] cutscene {event_id} auto-skipped ({})",
                            reason.as_str()
                        ),
                    },
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

/// Reactor-local model of the self player's in-flight action (see
/// `keepalive_loop`). `bar` is Some only for spells; instant JA/WS/ranged set
/// only the `lock_until` re-issue gate.
struct CastInFlight {
    lock_until: std::time::Instant,
    bar: Option<CastBar>,
}

/// Armed at send but not started: the bar waits for the server's own
/// BATTLE2 MagicStart so it cannot lead the cast pose and the "starts casting"
/// line by a round trip.
struct CastBar {
    name: String,
    total_ms: u32,
    started_at: Option<std::time::Instant>,
}

/// Drives the self cast bar from the server's own BATTLE2 action packets.
///
/// vendor/server/src/map/ai/states/magic_state.cpp:127 pushes the MagicStart
/// action_t from the `CMagicState` constructor, i.e. synchronously inside the
/// 0x1A action handler (player_controller.cpp:50-59 → ai_container.cpp:225-234),
/// so this packet is the server's cast-start instant and carries the same cast
/// pose and "starts casting" line. An interrupt reuses the MagicStart category
/// with an "sp*" FourCC (vendor/server/src/map/action/interrupts.cpp:268-284).
fn apply_self_battle2_to_cast(
    header: &Battle2Header,
    cast_in_flight: &mut Option<CastInFlight>,
    event_tx: &broadcast::Sender<AgentEvent>,
) {
    let now = std::time::Instant::now();
    let mut clear = false;
    if let Some(c) = cast_in_flight.as_mut() {
        if let Some(bar) = c.bar.as_mut() {
            let started = bar.started_at.is_some();
            let end = |interrupted: bool| {
                if started {
                    let _ = event_tx.send(AgentEvent::SelfCastEnded { interrupted });
                }
            };
            match header.action_kind {
                ffxi_vocab::magic::CATEGORY_MAGIC_START => {
                    let routine = ffxi_vocab::magic::magic_start_routine(header.action_id);
                    if routine.is_some_and(|r| r.interrupt) {
                        end(true);
                        clear = true;
                    } else if !started {
                        bar.started_at = Some(now);
                        let _ = event_tx.send(AgentEvent::SelfCastStarted {
                            name: bar.name.clone(),
                            total_ms: bar.total_ms,
                        });
                        // The gate was armed a round trip ago; without this the bar
                        // would be cut short by exactly that round trip.
                        c.lock_until = c
                            .lock_until
                            .max(now + std::time::Duration::from_millis(u64::from(bar.total_ms)));
                    }
                }
                ffxi_vocab::magic::CATEGORY_MAGIC_FINISH => {
                    end(false);
                    clear = true;
                }
                _ => {}
            }
        }
    }
    if clear {
        *cast_in_flight = None;
    }
}

#[allow(clippy::too_many_arguments)]
async fn keepalive_loop(
    map: &mut MapClient,
    mut sub_seq: u16,
    mut server_last_seq: u16,
    mut pending_event_end: Vec<(u32, u16, u16)>,
    mut cutscene: crate::event_dialog::CutsceneScope,
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
    mut sysmes_resolver: treasure::SysMesResolver,
    mut treasure_pool: treasure::TreasurePool,
    mut mog: SelfMogState,
    flood_zone_messages: Vec<(u16, Vec<u8>)>,
    spell_table: Option<std::sync::Arc<ffxi_dat::spell_info::SpellTable>>,
    // FFXI_SERVER host (session::Config::server); the search server listens
    // beside the auth/lobby ports there, not on the per-zone map address.
    server_host: String,
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

    let mut tick = tokio::time::interval(SESSION_TICK_PERIOD);
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

    for (opcode, body) in &flood_zone_messages {
        emit_zone_message_chat(
            *opcode,
            body,
            &mut dialog_session,
            current_zone_id,
            &character_name,
            &event_tx,
        );
    }

    let mut local_menu = crate::local_menu::LocalMenuSession::new();

    let mut dbox = crate::delivery_box::DeliveryBoxSession::default();

    let mut auction = crate::auction::AuctionFlow::default();
    let ah_search_inflight = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Delivery-box recipient entry: `awaiting_recipient` is set while the
    // name-entry frame is up (the next TextInput answers it);
    // `pending_recipient` holds the name sent in a 0x04D Query until its
    // PBX_RESULT RecipientCheck settles.
    let mut awaiting_recipient = false;
    let mut pending_recipient: Option<String> = None;
    // The recipient locked by a successful Query; the session injects it into
    // outgoing `Set` so the dedicated screen (and relay) never re-send it.
    let mut locked_recipient: Option<String> = None;

    // LOC_INVENTORY mirror for the delivery-box item picker:
    // slot -> (item_no, quantity, locked). Maintained inline from
    // ITEM_LIST/ITEM_ATTR/ITEM_NUM before they reach handle_sub_packet.
    let mut inv_mirror: std::collections::BTreeMap<u8, (u16, u32, bool)> =
        std::collections::BTreeMap::new();

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

    // Post-zone-in settle window (see ZONE_IN_SETTLE): for the first moments of a
    // zone-generation, refuse to snap self_pos to a far-away carrier so an
    // out-of-order/duplicate position from around the transition cannot drop us in
    // the wrong zone. Anchored at main-loop start — by then the flood has seeded (or
    // will shortly), and this covers the vulnerable period while late carriers can
    // still arrive.
    let zone_in_settle_until = std::time::Instant::now() + ZONE_IN_SETTLE;

    let mut self_in_mog_house = false;

    // In-flight self action: gates re-issuing another action until `lock_until`,
    // and (for spells) drives the cast bar via Self Cast* events. Optimistic on
    // send; cleared by the timer, by movement (spell interrupt), or on a fresh
    // action. See ActionKind::{cast_bar, action_lock_ms}.
    let mut cast_in_flight: Option<CastInFlight> = None;

    // Per-spell recast expiry, tracked entirely client-side: LSB's 0x119 sends
    // ability recasts only (RECAST_ABILITY), never magic, so the client owns
    // spell-recast timing from the scraped base recastTime — as retail's client
    // does. (Base recast doesn't model recast-down gear, so this can over-block
    // a hasted caster; refine when recast-down is modeled.)
    let mut spell_recast_until: std::collections::HashMap<u16, std::time::Instant> =
        std::collections::HashMap::new();

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                if let Some(c) = cmd.as_ref() {
                    tracing::debug!(variant = ?std::mem::discriminant(c), "cmd_rx recv");
                }
                match cmd {
                    None => break,
                    Some(AgentCommand::GroundCorrection { x, y, z, heading }) => {
                        // The player did not walk, so no cast is interrupted:
                        // this is the client repairing a wedged height that the
                        // server only ever echoes back (kuluu-mo4q).
                        self_pos = Position { pos: Vec3 { x, y, z }, heading, ..self_pos };
                        let _ = event_tx.send(AgentEvent::PositionChanged { pos: self_pos });
                    }
                    Some(AgentCommand::Move { x, y, z, heading }) => {
                        // Translation (not a turn-in-place) interrupts a spell in
                        // flight — retail cancels the cast when the caster moves.
                        // Instant JA/WS/ranged locks are unaffected.
                        let moved = (x - self_pos.pos.x).abs() > f32::EPSILON
                            || (y - self_pos.pos.y).abs() > f32::EPSILON
                            || (z - self_pos.pos.z).abs() > f32::EPSILON;
                        self_pos = Position { pos: Vec3 { x, y, z }, heading, ..self_pos };
                        let _ = event_tx.send(AgentEvent::PositionChanged { pos: self_pos });
                        if moved {
                            if let Some(c) = &cast_in_flight {
                                if let Some(bar) = &c.bar {
                                    if bar.started_at.is_some() {
                                        let _ = event_tx
                                            .send(AgentEvent::SelfCastEnded { interrupted: true });
                                    }
                                    cast_in_flight = None;
                                }
                            }
                        }
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
                        // VM-driven event: cancel — the VM reports the frame's
                        // cancel result and EVENT_END carries the cancel
                        // EndPara; the server script decides what it means
                        // (OnEventFinish result, vendor/server/src/map/packets/
                        // c2s/0x05b_eventend.cpp:36-70).
                        } else if let Some((u, a, n)) = dialog_session.active_end() {
                            let advance = dialog_session.cancel();
                            for cue in dialog_session.take_cues() {
                                cutscene.push(cue, &event_tx);
                            }
                            match advance {
                                crate::event_dialog::Advance::Frame(dialog) => {
                                    emit_event_speech_to_chat(&event_tx, &dialog);
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                                crate::event_dialog::Advance::Ended { end_para } => {
                                    if take_pending_event_end(&mut pending_event_end, u, n) {
                                        let payload = build_subpacket_event_end(sub_seq, u, a, current_zone_id, n, end_para);
                                        sub_seq = sub_seq.wrapping_add(1);
                                        if let Err(e) = map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq).await {
                                            tracing::warn!(error = %e, "EVENT_END (vm) send failed");
                                        }
                                    }
                                    cutscene.end(crate::event_dialog::EventSessionExit::Cancelled, &event_tx);
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                }
                                // Esc mid-wait: the VM defers the cancel to the
                                // next message wait; if none follows, the scene
                                // just plays out (kuluu-bxts: cancel latch).
                                crate::event_dialog::Advance::Waiting => {}
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
                            cutscene.end(crate::event_dialog::EventSessionExit::ScriptEnded, &event_tx);
                            let _ = event_tx.send(AgentEvent::EventEnded);
                        } else {
                            // Nothing outstanding server-side: every tracked
                            // event lives in dialog_session or
                            // pending_event_end, and LSB drops a 0x05B whose
                            // EventPara doesn't match currentEvent->eventId
                            // (vendor/server/src/map/packets/c2s/
                            // validation.cpp:58-77), so there is no valid
                            // event-finish to fabricate here.
                            cutscene.end(crate::event_dialog::EventSessionExit::ScriptEnded, &event_tx);
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
                                            spans: Vec::new(),
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
                                            spans: Vec::new(),
                                            channel: ChatChannel::System,
                                            sender: "<client>".into(),
                                            text: format!("[mog] Browsing {name}."),
                                            server_ts: 0,
                                        },
                                    });
                                }
                                crate::local_menu::Advance::DeliveryOpen { box_no } => {
                                    // Cutover: the dedicated screen (gated on the
                                    // snapshot's delivery_box) now owns the UI, so
                                    // open non-menu-driven — no legacy DialogState
                                    // grid re-render on settle.
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                    let op = dbox.request_open(box_no, false);
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
                                crate::local_menu::Advance::DeliveryRecipient { frame } => {
                                    // The next TextInput answers this frame.
                                    awaiting_recipient = true;
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog: frame });
                                }
                                crate::local_menu::Advance::DeliveryPut { slot } => {
                                    // Offer sendable LOC_INVENTORY stacks: skip Gil
                                    // (index 0), empty slots, equipped/locked gear,
                                    // and NoDelivery items without CanSendAccount
                                    // (dboxutils.cpp AddItemsToBeSent).
                                    let items: Vec<crate::local_menu::PickableItem> = inv_mirror
                                        .iter()
                                        .filter(|&(&inv_slot, &(item_no, quantity, locked))| {
                                            inv_slot != 0
                                                && item_no != 0
                                                && quantity != 0
                                                && !locked
                                                && ffxi_vocab::item_flags::deliverable(item_no)
                                        })
                                        .map(|(&inv_slot, &(item_no, quantity, _))| {
                                            crate::local_menu::PickableItem {
                                                inventory_slot: inv_slot,
                                                item_no,
                                                quantity,
                                            }
                                        })
                                        .collect();
                                    let dialog = local_menu.open_delivery_pick(slot, &items);
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                                crate::local_menu::Advance::Close => {
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                }
                            }
                        // VM-driven event: feed the selection to the script and
                        // advance; only send EVENT_END once it ends.
                        } else if let Some((u, a, n)) = dialog_session.active_end() {
                            let advance = dialog_session.advance(Some(choice));
                            for cue in dialog_session.take_cues() {
                                cutscene.push(cue, &event_tx);
                            }
                            match advance {
                                crate::event_dialog::Advance::Frame(dialog) => {
                                    emit_event_speech_to_chat(&event_tx, &dialog);
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                                crate::event_dialog::Advance::Ended { end_para } => {
                                    if take_pending_event_end(&mut pending_event_end, u, n) {
                                        let payload = build_subpacket_event_end(sub_seq, u, a, current_zone_id, n, end_para);
                                        sub_seq = sub_seq.wrapping_add(1);
                                        if let Err(e) = map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq).await {
                                            tracing::warn!(error = %e, "EVENT_END (vm choice) send failed");
                                        }
                                    }
                                    cutscene.end(crate::event_dialog::EventSessionExit::ScriptEnded, &event_tx);
                                    let _ = event_tx.send(AgentEvent::EventEnded);
                                }
                                crate::event_dialog::Advance::Waiting => {}
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

                            take_pending_event_end(&mut pending_event_end, event_id, event_num);
                            cutscene.end(crate::event_dialog::EventSessionExit::ScriptEnded, &event_tx);
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
                    Some(AgentCommand::CustomMenuRespond { title, option }) => {
                        // The customMenu is fully client-driven: the server holds
                        // the context and only awaits the `_CUSTOM_MENU` tell, so
                        // clear the prompt locally rather than waiting for an event.
                        let text = custom_menu_reply(&character_name, &title, option.as_deref());
                        tracing::info!(sub_seq, reply = %text, "custom menu reply send (0x0B6)");
                        let payload = build_subpacket_tell(sub_seq, CUSTOM_MENU_SENDER, &text);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "custom menu reply send failed");
                        }
                        cutscene.end(crate::event_dialog::EventSessionExit::ScriptEnded, &event_tx);
                        let _ = event_tx.send(AgentEvent::EventEnded);
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
                        // Retail client cast time comes from the spell DAT, not LSB.
                        let dat_cast_ms = match &kind {
                            crate::state::ActionKind::CastMagic { spell_id, .. } => spell_table
                                .as_ref()
                                .and_then(|t| t.lookup(*spell_id as u16))
                                .map(|s| s.cast_time_ms)
                                .filter(|ms| *ms > 0),
                            _ => None,
                        };
                        // Reject a fresh action while the previous one is still
                        // locked in (retail refuses the cast/ability rather than
                        // queueing it). Non-locking actions (Attack/Talk/…) pass.
                        if kind.action_lock_ms(dat_cast_ms).is_some() {
                            if let Some(c) = &cast_in_flight {
                                if std::time::Instant::now() < c.lock_until {
                                    let text = if matches!(
                                        kind,
                                        crate::state::ActionKind::CastMagic { .. }
                                    ) {
                                        "Unable to cast spells at this time."
                                    } else {
                                        "You must wait longer to perform that action."
                                    };
                                    let _ = event_tx.send(AgentEvent::ChatLine {
                                        line: ChatLine {
                                            spans: Vec::new(),
                                            channel: ChatChannel::System,
                                            sender: "<client>".into(),
                                            text: text.into(),
                                            server_ts: 0,
                                        },
                                    });
                                    continue;
                                }
                            }
                        }
                        // Reject a spell still on its client-tracked recast (LSB
                        // never sends magic recasts, so the client owns the timer).
                        if let crate::state::ActionKind::CastMagic { spell_id, .. } = &kind {
                            if let Some(&until) = spell_recast_until.get(&(*spell_id as u16)) {
                                if std::time::Instant::now() < until {
                                    let _ = event_tx.send(AgentEvent::ChatLine {
                                        line: ChatLine {
                                            spans: Vec::new(),
                                            channel: ChatChannel::System,
                                            sender: "<client>".into(),
                                            text: "Unable to cast spells at this time.".into(),
                                            server_ts: 0,
                                        },
                                    });
                                    continue;
                                }
                            }
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
                        } else {
                            let now = std::time::Instant::now();
                            // Start the client-side recast clock for a spell.
                            if let crate::state::ActionKind::CastMagic { spell_id, .. } = &kind {
                                if let Some(rms) = ffxi_vocab::cast_time::spell_recast_time_ms(
                                    *spell_id as u16,
                                ) {
                                    if rms > 0 {
                                        spell_recast_until.insert(
                                            *spell_id as u16,
                                            now + std::time::Duration::from_millis(u64::from(rms)),
                                        );
                                    }
                                }
                            }
                            // The re-issue gate is optimistic (it guards our own
                            // send), but the bar it carries only starts when the
                            // server echoes MagicStart — see start_self_cast_bar.
                            if let Some(lock_ms) = kind.action_lock_ms(dat_cast_ms) {
                                cast_in_flight = Some(CastInFlight {
                                    lock_until: now
                                        + std::time::Duration::from_millis(u64::from(lock_ms)),
                                    bar: kind.cast_bar(dat_cast_ms).map(|(name, total_ms)| {
                                        CastBar {
                                            name,
                                            total_ms,
                                            started_at: None,
                                        }
                                    }),
                                });
                            }
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
                    Some(AgentCommand::WidescanRequest) => {
                        let payload = build_subpacket_tracking_list(sub_seq);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "widescan list request send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("widescan request send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::WidescanTrack { act_index }) => {
                        let payload = build_subpacket_tracking_start(sub_seq, act_index);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "widescan track send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("widescan track send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::WidescanEnd) => {
                        let payload = build_subpacket_tracking_end(sub_seq);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "widescan end send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("widescan end send: {e}"),
                            });
                        }
                        // The server acknowledges c2s 0x0F6 with silence, not a
                        // 0x0F5 Lose, so the tracked state must clear locally
                        // like retail's client does — otherwise the compass
                        // pointer and map marker freeze on the last streamed
                        // position forever.
                        let _ = event_tx.send(AgentEvent::WidescanTrackUpdated { tracked: None });
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
                    | Some(AgentCommand::SetTargetLock { .. })
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
                    Some(AgentCommand::CancelBuff { icon }) => {
                        let payload = build_subpacket_buffcancel(sub_seq, icon);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "buffcancel send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("buff cancel send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::CheckTarget {
                        target_id,
                        target_index,
                        kind,
                    }) => {
                        if kind == crate::state::CheckKind::Check {
                            let _ = event_tx.send(AgentEvent::CheckCleared);
                        }
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
                    Some(AgentCommand::OpenBazaar {
                        target_id,
                        target_index,
                    }) => {
                        // LSB rejects a BAZAAR_LIST while we still hold a
                        // BazaarID, and its BAZAAR_EXIT handler clears that id
                        // unconditionally (0x104_bazaar_exit.cpp:59), so leaving
                        // first makes re-browsing safe even from a stale view.
                        let exit = build_subpacket_bazaar_exit(sub_seq);
                        sub_seq = sub_seq.wrapping_add(1);
                        let mut sent = map
                            .send_encrypted(&exit, datagram_header_id(sub_seq), server_last_seq)
                            .await;
                        if sent.is_ok() {
                            let payload =
                                build_subpacket_bazaar_list(sub_seq, target_id, target_index);
                            sub_seq = sub_seq.wrapping_add(1);
                            sent = map
                                .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                                .await;
                        }
                        match sent {
                            Ok(()) => {
                                let _ = event_tx.send(AgentEvent::BazaarOpened {
                                    seller_id: target_id,
                                    seller_index: target_index,
                                    seller_name: name_cache
                                        .get(&target_id)
                                        .cloned()
                                        .unwrap_or_default(),
                                });
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "bazaar_list send failed");
                                let _ = event_tx.send(AgentEvent::Error {
                                    message: format!("bazaar open send: {e}"),
                                });
                            }
                        }
                    }
                    Some(AgentCommand::BuyBazaarItem { index, quantity }) => {
                        let payload = build_subpacket_bazaar_buy(sub_seq, index, quantity);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "bazaar_buy send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("bazaar buy send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::CloseBazaar) => {
                        let payload = build_subpacket_bazaar_exit(sub_seq);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "bazaar_exit send failed");
                        }
                        let _ = event_tx.send(AgentEvent::BazaarClosed);
                    }
                    Some(AgentCommand::AhBrowse { category, sorts }) => {
                        spawn_ah_search(
                            &ah_search_inflight,
                            &event_tx,
                            server_host.clone(),
                            AhSearchQuery::Browse { category, sorts },
                        );
                    }
                    Some(AgentCommand::AhHistory { item_id, stack }) => {
                        spawn_ah_search(
                            &ah_search_inflight,
                            &event_tx,
                            server_host.clone(),
                            AhSearchQuery::History { item_id, stack },
                        );
                    }
                    Some(AgentCommand::AhBid {
                        item_id,
                        stack,
                        price,
                    }) => {
                        // LSB's PacketValidator drops the whole 0x04E when
                        // BidPrice is outside 1..=999_999_999 — no reply ever
                        // comes back, so reject it here instead.
                        if !auc_price_valid(price, &event_tx) {
                            continue;
                        }
                        let payload = build_subpacket_auc_bid(
                            sub_seq,
                            price,
                            item_id,
                            crate::auction::stacks_wire(stack),
                            crate::auction::AUCTION_BID_WORK_INDEX,
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        match map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            Ok(()) => {
                                let _ = event_tx.send(AgentEvent::AuctionOpStarted {
                                    op: crate::state::AuctionBusy::PlacingBid,
                                });
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "auc bid send failed");
                                let _ = event_tx.send(AgentEvent::Error {
                                    message: format!("auction bid send: {e}"),
                                });
                            }
                        }
                    }
                    Some(AgentCommand::AhSell {
                        inventory_slot,
                        stack,
                        price,
                    }) => match inv_mirror.get(&inventory_slot) {
                        // Commission shares BidPrice's validator range; see AhBid.
                        Some(_) if !auc_price_valid(price, &event_tx) => {}
                        Some(&(item_no, _, _)) => {
                            let sell = auction.request_sell(inventory_slot, item_no, stack, price);
                            let payload = build_subpacket_auc_ask_commit(
                                sub_seq,
                                sell.price,
                                sell.inventory_slot as u16,
                                sell.item_no,
                                crate::auction::stacks_wire(sell.stack),
                            );
                            sub_seq = sub_seq.wrapping_add(1);
                            if let Err(e) = map
                                .send_encrypted(
                                    &payload,
                                    datagram_header_id(sub_seq),
                                    server_last_seq,
                                )
                                .await
                            {
                                tracing::warn!(error = %e, "auc ask_commit send failed");
                                let _ = event_tx.send(AgentEvent::Error {
                                    message: format!("auction sell send: {e}"),
                                });
                            }
                        }
                        None => {
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!(
                                    "auction sell: no item in inventory slot {inventory_slot}"
                                ),
                            });
                        }
                    },
                    Some(AgentCommand::AhSellConfirm) => match auction.confirm_sell() {
                        Some((sell, work_index)) => {
                            let payload = build_subpacket_auc_lot_in(
                                sub_seq,
                                sell.price,
                                sell.inventory_slot as u16,
                                crate::auction::stacks_wire(sell.stack),
                                work_index,
                            );
                            sub_seq = sub_seq.wrapping_add(1);
                            if let Err(e) = map
                                .send_encrypted(
                                    &payload,
                                    datagram_header_id(sub_seq),
                                    server_last_seq,
                                )
                                .await
                            {
                                tracing::warn!(error = %e, "auc lot_in send failed");
                                let _ = event_tx.send(AgentEvent::Error {
                                    message: format!("auction sell confirm send: {e}"),
                                });
                            }
                        }
                        None => {
                            let _ = event_tx.send(AgentEvent::Error {
                                message: "auction sell confirm: no fee quote pending".into(),
                            });
                        }
                    },
                    Some(AgentCommand::AhSalesStatus) => {
                        let payload = build_subpacket_auc_info(sub_seq);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "auc info send failed");
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("auction sales status send: {e}"),
                            });
                        }
                    }
                    Some(AgentCommand::AhCancelSale { slot }) => {
                        if (slot as usize) >= crate::state::AUCTION_SLOTS {
                            let _ = event_tx.send(AgentEvent::Error {
                                message: format!("auction cancel: slot {slot} out of range"),
                            });
                        } else {
                            let payload = build_subpacket_auc_lot_cancel(sub_seq, slot as i8);
                            sub_seq = sub_seq.wrapping_add(1);
                            if let Err(e) = map
                                .send_encrypted(
                                    &payload,
                                    datagram_header_id(sub_seq),
                                    server_last_seq,
                                )
                                .await
                            {
                                tracing::warn!(error = %e, "auc lot_cancel send failed");
                                let _ = event_tx.send(AgentEvent::Error {
                                    message: format!("auction cancel send: {e}"),
                                });
                            }
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
                    // Lot and pass are fire-and-forget: the server answers with
                    // 0x0D3, and silently ignores a repeat on a slot this
                    // character already acted on
                    // (vendor/server/src/map/packets/c2s/0x041_trophy_entry.cpp).
                    // Fire-and-forget: the server has nothing to answer with,
                    // it just saves PChar->loc.boundary
                    // (vendor/server/src/map/packets/c2s/0x0f2_submapchange.cpp).
                    Some(AgentCommand::ReportSubArea { sub_area }) => {
                        let payload = build_subpacket_submapchange(
                            sub_seq,
                            ffxi_proto::map::submap::state::GENERAL,
                            sub_area,
                        );
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "submapchange send failed");
                        }
                    }
                    Some(AgentCommand::TreasureLot { slot }) => {
                        let payload = build_subpacket_trophy_lot(sub_seq, slot);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "treasure lot send failed");
                        }
                    }
                    Some(AgentCommand::TreasurePass { slot }) => {
                        let payload = build_subpacket_trophy_pass(sub_seq, slot);
                        sub_seq = sub_seq.wrapping_add(1);
                        if let Err(e) = map
                            .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                            .await
                        {
                            tracing::warn!(error = %e, "treasure pass send failed");
                        }
                    }
                    Some(AgentCommand::TextInput { text }) => {
                        // Only the delivery-box recipient prompt takes free
                        // text; ignore stray input otherwise.
                        if awaiting_recipient {
                            awaiting_recipient = false;
                            let name = text.trim().to_string();
                            if name.is_empty() {
                                // Cleared entry: unlock any recipient and
                                // re-render the Send panel.
                                if let Some(dialog) = local_menu.set_recipient(None) {
                                    let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                                }
                            } else {
                                // Verify the name server-side (0x04D Query)
                                // before locking it; PBX_RESULT settles it.
                                pending_recipient = Some(name.clone());
                                let op = crate::state::DeliveryBoxOp::Query { recipient: name };
                                send_pbx(map, &op, &mut sub_seq, server_last_seq, &event_tx).await;
                            }
                        }
                    }
                    Some(AgentCommand::DeliveryBox { op }) => {
                        // Track menu_driven=false so agent-driven flows don't
                        // re-render dialog menus on settle.
                        let mut op = op;
                        match &op {
                            crate::state::DeliveryBoxOp::PostOpen => {
                                dbox.request_open(crate::state::DeliveryBoxNo::Incoming, false);
                                locked_recipient = None;
                            }
                            crate::state::DeliveryBoxOp::DeliOpen => {
                                dbox.request_open(crate::state::DeliveryBoxNo::Outgoing, false);
                                locked_recipient = None;
                            }
                            crate::state::DeliveryBoxOp::PostClose { .. } => {
                                locked_recipient = None;
                            }
                            _ => {}
                        }
                        // The recipient Query records the name so RecipientCheck
                        // settles it, and projects "(checking…)" to the screen.
                        if let crate::state::DeliveryBoxOp::Query { recipient } = &op {
                            let name = recipient.clone();
                            pending_recipient = Some(name.clone());
                            let _ = event_tx.send(AgentEvent::DeliveryBoxUpdated {
                                box_no: crate::state::DeliveryBoxNo::Outgoing,
                                update: crate::state::DeliveryBoxUpdate::RecipientPending { name },
                            });
                        }
                        // Inject the session-authoritative locked recipient so the
                        // viewer never has to re-supply it on Set.
                        if let crate::state::DeliveryBoxOp::Set { recipient, .. } = &mut op {
                            if let Some(locked) = &locked_recipient {
                                *recipient = locked.clone();
                            }
                        }
                        send_pbx(map, &op, &mut sub_seq, server_last_seq, &event_tx).await;
                    }
                    Some(AgentCommand::DeliveryTake { slot }) => {
                        // Accept→Get chain (Get depends on the Accept ack).
                        let op = dbox.request_take(slot);
                        send_pbx(map, &op, &mut sub_seq, server_last_seq, &event_tx).await;
                    }
                    Some(AgentCommand::DebugDrive { .. })
                    | Some(AgentCommand::DebugHeights)
                    | Some(AgentCommand::Screenshot { .. }) => {
                        // GUI-only debug driving: consumed by the native input
                        // path via DebugControlHandle (the agent-socket decoder),
                        // never the network session. No-op here (headless/mcp).
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
                                        spans: Vec::new(),
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

                // Carry a scene holding on a timed wait (0x1C/0x6F). Without
                // this the VM runs a cutscene to its end in the tick the player
                // answers, and every cue lands on one frame.
                if let Some((u, a, n)) = dialog_session.active_end() {
                    let advance = dialog_session.tick(SESSION_TICK_PERIOD.as_secs_f32());
                    for cue in dialog_session.take_cues() {
                        cutscene.push(cue, &event_tx);
                    }
                    match advance {
                        crate::event_dialog::Advance::Frame(dialog) => {
                            emit_event_speech_to_chat(&event_tx, &dialog);
                            let _ = event_tx.send(AgentEvent::EventDialog { dialog });
                        }
                        crate::event_dialog::Advance::Ended { end_para } => {
                            if take_pending_event_end(&mut pending_event_end, u, n) {
                                let payload = build_subpacket_event_end(sub_seq, u, a, current_zone_id, n, end_para);
                                sub_seq = sub_seq.wrapping_add(1);
                                if let Err(e) = map.send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq).await {
                                    tracing::warn!(error = %e, "EVENT_END (vm wait) send failed");
                                }
                            }
                            cutscene.end(crate::event_dialog::EventSessionExit::ScriptEnded, &event_tx);
                            let _ = event_tx.send(AgentEvent::EventEnded);
                        }
                        crate::event_dialog::Advance::Waiting => {}
                    }
                }

                // Advance the cast bar and clear the action lock when it expires.
                if let Some(c) = &cast_in_flight {
                    let now = std::time::Instant::now();
                    let started = c.bar.as_ref().and_then(|b| b.started_at.map(|t| (b, t)));
                    if let Some((bar, started_at)) = started {
                        let elapsed = started_at.elapsed().as_millis() as u32;
                        let _ = event_tx.send(AgentEvent::SelfCastProgress {
                            elapsed_ms: elapsed.min(bar.total_ms),
                        });
                    }
                    if now >= c.lock_until {
                        if started.is_some() {
                            let _ = event_tx
                                .send(AgentEvent::SelfCastEnded { interrupted: false });
                        }
                        cast_in_flight = None;
                    }
                }

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
                                spans: Vec::new(),
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
                            EventTrigger {
                                event_zone: ev.event_num,
                                text_zone: ev.event_num,
                                unique_no: self_char_id,
                                act_index: self_act_index.unwrap_or(0),
                                event_id: ev.event_para,
                                params: Vec::new(),
                                npc_name: None,
                            },
                            &event_tx,
                            &mut cutscene,
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

                if let Some(flush) = flush_pending_event_end(
                    EventEndFlushInputs {
                        user_driven: user_driven_events,
                        watchdog_fires,
                        walked_away,
                    },
                    &mut pending_event_end,
                    dialog_session.active_end(),
                    current_zone_id,
                    sub_seq,
                ) {
                    payload.extend(flush.payload);
                    sub_seq = flush.next_sub_seq;
                    for _ in 0..flush.released {
                        cutscene.end(crate::event_dialog::EventSessionExit::WatchdogReleased, &event_tx);
                        let _ = event_tx.send(AgentEvent::EventEnded);
                    }
                    if flush.clear_dialog {
                        dialog_session.clear();
                    }
                    if walked_away {
                        tracing::info!(
                            moved_yalms = walk_dist.unwrap_or(0.0),
                            "released pinned event: player walked away from dialog"
                        );
                        let _ = event_tx.send(AgentEvent::ChatLine {
                            line: ChatLine {
                                spans: Vec::new(),
                                channel: ChatChannel::System,
                                sender: "<client>".into(),
                                text: "Released the pinned event (you walked away from it)."
                                    .into(),
                                server_ts: 0,
                            },
                        });
                    } else if watchdog_fires {
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
                                    cutscene.end(
                                        crate::event_dialog::EventSessionExit::ZoneChanged,
                                        &event_tx,
                                    );
                                    let _ = event_tx.send(AgentEvent::ZoneChanged {
                                        from: None,
                                        to: kuluu_snapshot::ZONE_UNKNOWN,
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
                                        // Settle a pending recipient Query: an OK
                                        // check locks the name into the Send panel
                                        // (re-rendered with slots activated); a miss
                                        // drops it (dbox already emits the notice).
                                        for (_, update) in &out.updates {
                                            if let crate::state::DeliveryBoxUpdate::RecipientCheck {
                                                ok,
                                                ..
                                            } = update
                                            {
                                                if let Some(name) = pending_recipient.take() {
                                                    // Authoritative locked recipient the
                                                    // session injects into Set (so neither
                                                    // viewer nor relay must re-send it).
                                                    locked_recipient =
                                                        ok.then(|| name.clone());
                                                    // Legacy DialogState re-render only for
                                                    // the old menu-driven path; the dedicated
                                                    // screen reads recipient from the snapshot.
                                                    if dbox.menu_driven() {
                                                        let dialog = if *ok {
                                                            local_menu.set_recipient(Some(name))
                                                        } else {
                                                            local_menu.set_recipient(None)
                                                        };
                                                        if let Some(dialog) = dialog {
                                                            let _ = event_tx.send(
                                                                AgentEvent::EventDialog { dialog },
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        for (box_no, update) in out.updates {
                                            let _ = event_tx.send(
                                                AgentEvent::DeliveryBoxUpdated { box_no, update },
                                            );
                                        }
                                        for text in out.notices {
                                            let _ = event_tx.send(AgentEvent::ChatLine {
                                                line: ChatLine {
                                                    spans: Vec::new(),
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

                            if sub.opcode == ffxi_proto::map::s2c::AUC {
                                match decode::Auction::decode(sub.data) {
                                    Ok(a) => {
                                        let out = auction.on_packet(&a);
                                        for ev in out.events {
                                            let _ = event_tx.send(ev);
                                        }
                                        if out.send_work_check {
                                            let payload = build_subpacket_auc_work_check(sub_seq);
                                            sub_seq = sub_seq.wrapping_add(1);
                                            if let Err(e) = map
                                                .send_encrypted(&payload, datagram_header_id(sub_seq), server_last_seq)
                                                .await
                                            {
                                                tracing::warn!(error = %e, "auc work_check send failed");
                                            }
                                        }
                                    }
                                    Err(e) => warn_decode_err(sub.opcode, e),
                                }
                                continue;
                            }

                            // The TALKNUM family resolves against the zone
                            // dialog DAT that dialog_session owns, so it can't
                            // live in handle_sub_packet.
                            if ZONE_MESSAGE_OPCODES.contains(&sub.opcode) {
                                emit_zone_message_chat(
                                    sub.opcode,
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
                                if let Some(mut trigger) = event_trigger(&sub) {
                                    let unique_no = trigger.unique_no;
                                    trigger.npc_name =
                                        name_cache.get(&unique_no).cloned().or_else(|| {
                                            npc_name_resolver
                                                .lookup(unique_no)
                                                .map(|s| s.replace('_', " "))
                                        });
                                    begin_server_event(
                                        &mut dialog_session,
                                        trigger,
                                        &event_tx,
                                        &mut cutscene,
                                        &mut pending_event_end,
                                        &mut auto_event_end,
                                    );
                                    continue;
                                }
                            }

                            if sub.opcode == ffxi_proto::map::s2c::EVENTUCOFF
                                && eventucoff_mode_of(sub.data)
                                    == Some(ffxi_proto::map::eventucoff_mode::CANCEL_EVENT)
                            {
                                dialog_session.clear();
                            }

                            // Keep the LOC_INVENTORY mirror for the delivery-box
                            // item picker current (category 0; index 0 is Gil).
                            if sub.opcode == ffxi_proto::map::s2c::ITEM_LIST {
                                if let Ok(l) = decode::ItemList::decode(sub.data) {
                                    if l.category == 0 {
                                        if l.item_no == 0 || l.quantity == 0 {
                                            inv_mirror.remove(&l.index);
                                        } else {
                                            inv_mirror.insert(
                                                l.index,
                                                (l.item_no, l.quantity, l.lock_flg != 0),
                                            );
                                        }
                                    }
                                }
                            } else if sub.opcode == ffxi_proto::map::s2c::ITEM_ATTR {
                                if let Ok(a) = decode::ItemAttr::decode(sub.data) {
                                    if a.category == 0 {
                                        if a.item_no == 0 || a.quantity == 0 {
                                            inv_mirror.remove(&a.index);
                                        } else {
                                            inv_mirror.insert(
                                                a.index,
                                                (a.item_no, a.quantity, a.lock_flg != 0),
                                            );
                                        }
                                    }
                                }
                            } else if sub.opcode == ffxi_proto::map::s2c::ITEM_NUM {
                                if let Ok(n) = decode::ItemNum::decode(sub.data) {
                                    if n.category == 0 {
                                        if n.quantity == 0 {
                                            inv_mirror.remove(&n.index);
                                        } else if let Some(e) = inv_mirror.get_mut(&n.index) {
                                            e.1 = n.quantity;
                                            e.2 = n.lock_flg != 0;
                                        }
                                    }
                                }
                            }

                            let prev_self_pos = self_pos.pos;
                            handle_sub_packet(
                                &sub,
                                &event_tx,
                                &mut pending_event_end,
                                &mut cutscene,
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
                                &mut sysmes_resolver,
                                &mut treasure_pool,
                                &mut self_in_mog_house,
                                &mut mog,
                                None,
                            );

                            if sub.opcode == ffxi_proto::map::s2c::BATTLE2 {
                                if let Some(h) = decode_battle2_header(sub.data) {
                                    if h.actor_id == self_char_id {
                                        apply_self_battle2_to_cast(
                                            &h,
                                            &mut cast_in_flight,
                                            &event_tx,
                                        );
                                    }
                                }
                            }

                            if sub.opcode == ffxi_proto::map::s2c::CHAR_PC {
                                if let Ok(head) = decode::PosHead::decode(sub.data) {
                                    if head.unique_no == self_char_id {
                                        let server_pos = self_pos.pos;
                                        // During the post-zone-in settle window a far (>snap)
                                        // carrier is an out-of-order/duplicate position from around
                                        // the transition, not a real teleport — keep our local seed
                                        // (see ZONE_IN_SETTLE).
                                        let refuse_snap = self_pos_seeded
                                            && std::time::Instant::now() < zone_in_settle_until;
                                        match reconcile_self_pos(prev_self_pos, server_pos, refuse_snap) {
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

    // The map session outlives no cutscene: the next one starts with a fresh
    // `CutsceneScope`, so anything still held has to be released here or it
    // would never be (an event body that locked the camera and never issued
    // 0x46 case 0 is the common case, not the exception).
    cutscene.end(
        crate::event_dialog::EventSessionExit::Disconnected,
        &event_tx,
    );

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

/// The s2c opcodes that print a zone-dialog string: TALKNUMWORK2, TALKNUMWORK,
/// TALKNUM, TALKNUMNAME. They differ only in which parameters they carry, so
/// [`emit_zone_message_chat`] handles all four. LSB routes every fishing line
/// through this family (vendor/server/src/map/utils/fishingutils.cpp).
pub(crate) const ZONE_MESSAGE_OPCODES: [u16; 4] = [
    ffxi_proto::map::s2c::TALKNUMWORK2,
    ffxi_proto::map::s2c::TALKNUMWORK,
    ffxi_proto::map::s2c::TALKNUM,
    ffxi_proto::map::s2c::TALKNUMNAME,
];

/// One TALKNUM-family message flattened to what rendering a chat line needs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ZoneMessage {
    message_index: u16,
    /// The attributed speaker, already resolved against the hide-name flag —
    /// `None` when retail would print the line unattributed.
    speaker: Option<String>,
    /// The name the dialog string's text params (`{ChocoboName:N}`) resolve
    /// to — the angler on LSB's catch broadcasts. Kept apart from `speaker`:
    /// LSB's fishing constructor sets the hide-name flag precisely because the
    /// dialog string embeds this name itself.
    actor: Option<String>,
    nums: Vec<i32>,
}

/// Decode one TALKNUM-family body and emit its chat line. Shared by the
/// keepalive receive path and the zone-in flood replay so both honor the
/// never-drop-silently invariant of [`zone_message_chat_line`].
fn emit_zone_message_chat(
    opcode: u16,
    body: &[u8],
    dialog_session: &mut crate::event_dialog::DialogSession,
    zone_id: u16,
    character_name: &str,
    event_tx: &broadcast::Sender<AgentEvent>,
) {
    use ffxi_proto::map::s2c;

    let decoded = match opcode {
        s2c::TALKNUMWORK => decode::TalkNumWork::decode(body).map(|t| ZoneMessage {
            message_index: t.message_index(),
            speaker: (!t.hide_name()).then(|| t.speaker_name()).flatten(),
            actor: t.speaker_name(),
            nums: t.num.to_vec(),
        }),
        // 0x036 carries no name field: retail resolves the entity from
        // UniqueNo, and for every fishing line that is the angler whose name
        // the dialog string already embeds.
        s2c::TALKNUM => decode::TalkNum::decode(body).map(|t| ZoneMessage {
            message_index: t.message_index(),
            speaker: None,
            actor: None,
            nums: Vec::new(),
        }),
        s2c::TALKNUMWORK2 => decode::TalkNumWork2::decode(body).map(|t| ZoneMessage {
            message_index: t.message_index(),
            speaker: (!t.hide_name()).then(|| t.actor_name()).flatten(),
            actor: t.actor_name(),
            // Retail addresses both parameter banks through one index space.
            nums: t.num1.iter().chain(t.num2.iter()).copied().collect(),
        }),
        s2c::TALKNUMNAME => decode::TalkNumName::decode(body).map(|t| ZoneMessage {
            message_index: t.message_index(),
            speaker: (!t.hide_name()).then(|| t.actor_name()).flatten(),
            actor: t.actor_name(),
            nums: Vec::new(),
        }),
        _ => return,
    };

    use crate::event_dialog::FishingChat;
    match decoded {
        Ok(msg) => {
            // Fishing lines resolve against the DAT-located fishing block,
            // reconciling server/install client-era skew; anything else takes
            // the direct lookup.
            let (zone_text, size) =
                match dialog_session.fishing_chat(zone_id, msg.message_index, opcode) {
                    FishingChat::Line { text, offset } => (Some(text), fish_size_of_offset(offset)),
                    // Unresolved: a guess would print another line entirely, so
                    // the text degrades to the placeholder — but the mini-game
                    // bar label keeps the pin-era classification, correct for the
                    // dev stack and era-matched servers.
                    FishingChat::Unresolved => (None, hooked_fish_size(zone_id, msg.message_index)),
                    FishingChat::NotFishing => (
                        dialog_session.zone_chat_text(zone_id, msg.message_index as usize),
                        hooked_fish_size(zone_id, msg.message_index),
                    ),
                };
            if let Some(size) = size {
                let _ = event_tx.send(AgentEvent::FishHookedSize { size });
            }
            let _ = event_tx.send(AgentEvent::ChatLine {
                line: zone_message_chat_line(&msg, zone_text, character_name),
            });
        }
        Err(e) => warn_decode_err(opcode, &e),
    }
}

/// The hooked-fish size a zone message announces, if it announces one. LSB
/// pushes this TALKNUM immediately before 0x115, so the mini-game bar is
/// labelled by the time it appears
/// (vendor/server/src/map/utils/fishingutils.cpp `SendHookResponse`).
fn hooked_fish_size(zone_id: u16, mes_num: u16) -> Option<crate::state::FishSize> {
    ffxi_proto::fishing_messages::classify(zone_id, mes_num).and_then(fish_size_of_offset)
}

/// The bar label for a resolved FISHMESSAGEOFFSET, if it is one of the two
/// "something caught the hook" lines.
fn fish_size_of_offset(offset: u8) -> Option<crate::state::FishSize> {
    use ffxi_proto::fishing_messages::kind;
    match offset {
        kind::HOOKED_SMALL_FISH => Some(crate::state::FishSize::Small),
        kind::HOOKED_LARGE_FISH => Some(crate::state::FishSize::Large),
        _ => None,
    }
}

/// Render a TALKNUM-family message as a chat line: the zone dialog DAT entry at
/// `message_index` with the packet's params substituted ({Num:N}, {KeyItem:N},
/// {Item:N}). Degrades to a placeholder when the zone's string DAT is
/// unavailable — the message must never drop silently.
fn zone_message_chat_line(
    msg: &ZoneMessage,
    zone_text: Option<String>,
    player_name: &str,
) -> ChatLine {
    let speaker = msg.speaker.clone();
    let channel = if speaker.is_some() {
        ChatChannel::Say
    } else {
        ChatChannel::System
    };
    let Some(raw) = zone_text else {
        return ChatLine {
            spans: Vec::new(),
            channel,
            sender: speaker.unwrap_or_default(),
            text: format!(
                "[zone message {} — no printable dialog entry; params {:?}]",
                msg.message_index, msg.nums,
            ),
            server_ts: 0,
        };
    };

    // Item names stay their own span so the HUD can colour them apart, the way
    // retail does for every line that substitutes one — the fished-up catch
    // included (`.agents/skills/retail-observe/references/treasure-pool-chat.md`).
    let substituted = crate::event_dialog::substitute_nums(
        crate::event_dialog::substitute_text_params(
            crate::event_dialog::substitute_names(
                ffxi_event::clean_display(&raw, &msg.nums),
                player_name,
                speaker.as_deref(),
            ),
            msg.actor.as_deref(),
        ),
        &msg.nums,
    );
    let spans: Vec<crate::state::ChatSpan> =
        crate::event_dialog::spanned_entity_names(&substituted, &msg.nums)
            .into_iter()
            .map(|s| crate::state::ChatSpan {
                text: s.text,
                kind: match s.kind {
                    ffxi_dat::sysmes::SpanKind::Text => crate::state::ChatSpanKind::Text,
                    ffxi_dat::sysmes::SpanKind::Item => crate::state::ChatSpanKind::Item,
                    ffxi_dat::sysmes::SpanKind::KeyItem => crate::state::ChatSpanKind::KeyItem,
                },
            })
            .collect();

    ChatLine {
        sender: speaker.unwrap_or_default(),
        ..ChatLine::spanned(channel, spans)
    }
}

fn build_system_message_line(m: decode::SystemMessage) -> ChatLine {
    let text = match ffxi_vocab::msg_system::lookup(m.message_id) {
        Some(raw) => substitute_system_placeholders(raw, m.para, m.para2),
        None => format!("[system] msg #{} para={},{}", m.message_id, m.para, m.para2),
    };
    ChatLine {
        spans: Vec::new(),
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
            spans: Vec::new(),
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
        spans: Vec::new(),
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

// vendor/server/src/map/packets/s2c/0x028_battle2.cpp:41-58 — header packs actorId(32),
// trg_sum(6), res_sum(4), cmd_no(4), cmd_arg(32), info(32), then per target actorId(32) +
// resultCount(4). Only the first target is read: walking the rest requires re-walking the
// variable-length result blocks, and XIM likewise attaches to `context.primaryTargetId`
// (ParticleGeneratorAttachment.kt:75).
pub struct Battle2Header {
    pub actor_id: u32,
    pub action_id: u32,
    pub action_kind: u8,
    pub primary_target_id: Option<u32>,

    // 0x028_battle2.cpp:71-73 — the first result block's resolution(3), kind(2, skipped), and
    // animation(12). Decoded only for `CATEGORY_BASIC_ATTACK`: there `animation` is the swing
    // slot (attack.h:52-59) and these are the only per-swing data — which arm swung and whether
    // it landed. For any other category `animation` is a skill/anim id whose low values would
    // decode as a fabricated swing, so this stays `None`. A target-less or truncated body
    // carries no result at all.
    pub first_result: Option<ffxi_proto::melee::MeleeResult>,

    // The same 12 `animation` bits, uninterpreted. For every non-attack category this is the
    // index the caster's effect DAT is keyed by — LSB fills it from the spell's/ability's/
    // weapon skill's own animation column (charentity.cpp:1923, 1602; magic_state.cpp), which
    // is what the client resolves against its file table rather than the action id.
    pub animation: Option<u16>,
}

pub fn decode_battle2_header(data: &[u8]) -> Option<Battle2Header> {
    let mut br = BattleBitReader::new(data, 8);
    let actor_id = br.read(32)? as u32;
    let trg_sum = br.read(6)?;
    let _res_sum = br.read(4)?;
    let action_kind = br.read(4)? as u8;
    let action_id = br.read(32)? as u32;
    let _info = br.read(32);
    // LSB rounds the packet to a 4-byte size (basic.h:118), so a target carrying no results can
    // end the body before these trailing reads. Degrade to "no target" rather than dropping the
    // whole action — the sibling decode_battle2_action tolerates the same short payload.
    let primary_target_id = br.read(32).filter(|_| trg_sum > 0).map(|id| id as u32);
    let first = primary_target_id
        .and_then(|_| br.read(4))
        .filter(|count| *count > 0)
        .and_then(|_| {
            let resolution = br.read(3)? as u8;
            br.read(2)?;
            let animation = br.read(12)? as u16;
            Some((resolution, animation))
        });
    let first_result = first
        .filter(|_| action_kind == ffxi_proto::melee::CATEGORY_BASIC_ATTACK)
        .and_then(|(resolution, animation)| {
            ffxi_proto::melee::MeleeResult::from_wire(resolution, animation)
        });
    Some(Battle2Header {
        actor_id,
        action_id,
        action_kind,
        primary_target_id,
        first_result,
        animation: first.map(|(_, animation)| animation),
    })
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
        spans: Vec::new(),
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
    ffxi_vocab::msg_basic::lookup(message_num)
}

// vendor/server/src/map/enums/msg_std.h:48 — "<name> examines you.", sent to
// the checked PC via 0x009 (vendor/server/src/map/packets/c2s/0x0dd_equip_inspect.cpp:131).
const MSG_STD_EXAMINE: u16 = 89;

// s2c 0x009 GP_SERV_COMMAND_MESSAGE body: UniqueNo u32 @0, ActIndex u16 @4,
// MesNo u16 @6, Attr u8 @8 (vendor/server/src/map/packets/s2c/0x009_message.h:37-41).
fn decode_std_message_examine(
    data: &[u8],
    name_cache: &std::collections::HashMap<u32, String>,
) -> Option<ChatLine> {
    const UNIQUE_NO_OFFSET: usize = 0;
    const MES_NO_OFFSET: usize = 6;
    let unique_no = u32::from_le_bytes(
        data.get(UNIQUE_NO_OFFSET..UNIQUE_NO_OFFSET + 4)?
            .try_into()
            .ok()?,
    );
    let mes_no = u16::from_le_bytes(
        data.get(MES_NO_OFFSET..MES_NO_OFFSET + 2)?
            .try_into()
            .ok()?,
    );
    if mes_no != MSG_STD_EXAMINE {
        return None;
    }
    let name = name_for_id(unique_no, name_cache);
    Some(ChatLine {
        spans: Vec::new(),
        channel: ChatChannel::System,
        sender: name.clone(),
        text: format!("{name} examines you."),
        server_ts: 0,
    })
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

// ffxi_vocab::msg_basic is scraped from the trailing comment on each msg_basic.h enumerator,
// which fails the client two ways. Id 116 (the generic "uses <ability>" line shared by
// no-numeric buff JAs like Boost id39 / Warcry id32; abilities.sql message1=116) has no
// enumerator at all, so lookup(116) is None and the self-JA battle line goes missing. And
// wherever LSB's comment writes a bare ".." instead of a named token, the scrape has no way
// to know which value belongs there — 100/101 are what an ability with message1=0 falls back
// to (charentity.cpp:1945), so every plain job ability logged "<player> uses ..".
// Retail's full mesbasic table (ROM/27/72.DAT) carries the real strings; until that is
// scraped, pin the wording here. vendor/server/src/map/enums/msg_basic.h:46-185.
// The 420-427 Corsair roll family is deliberately absent: those need two numbers and a
// status effect that data1/data2 alone cannot supply.
const TEMPLATE_OVERRIDES: &[(u16, &str)] = &[
    (
        14,
        "The <player>'s attack is countered by the <target>. <number> of <player>'s shadows absorbs the damage and disappears.",
    ),
    (
        31,
        "<number> of <target>'s shadows absorb the damage and disappears.",
    ),
    (100, "The <player> uses <ability>."),
    (101, "The <player> uses <ability>."),
    (
        102,
        "The <player> uses <ability>. <target> recovers <number> HP.",
    ),
    (
        103,
        "The <player> uses <ability>. <target> recovers <number> HP.",
    ),
    (116, "<player> uses <ability>."),
    (
        136,
        "The <player> uses <ability>. <target> is now under the <player>'s control.",
    ),
    (
        137,
        "The <player> uses <ability>. The <player> fails to charm <target>.",
    ),
    (
        317,
        "The <player> uses <ability>. <target> takes <number> points of damage.",
    ),
    (324, "The <player> uses <ability>, but misses <target>."),
    // 565 (MsgBasic::Obtains) is only ever sent with gil as the argument —
    // vendor/server/src/map/utils/charutils.cpp:4756 (party split) and :4763
    // (solo) both push gil into GP_SERV_COMMAND_BATTLE_MESSAGE, and
    // vendor/server/scripts/globals/regimes.lua:1480 names it FOV_OBTAINS_GIL —
    // so the scraped "<target> obtains <amount>." is rendered with the gil
    // unit (retail wording tracked in kuluu-us91).
    (565, "<target> obtains <amount> gil."),
];

// Ids above that intentionally shadow a scraped msg_basic entry: for all but
// 565 the LSB enumerator comment elides tokens as a bare "..", so the scrape
// carries an unusable template (see the WHY atop TEMPLATE_OVERRIDES); 565's
// reason is on its entry. Guarded by
// tests::template_overrides_only_shadow_msg_basic_deliberately.
#[cfg(test)]
const DELIBERATE_SHADOWS: &[u16] = &[14, 31, 100, 101, 102, 103, 136, 137, 317, 324, 565];

fn subject_is_tar(message_num: u16) -> bool {
    matches!(message_num, 97)
}

// The `<skill>` token is overloaded across msg_basic. These three are the skill-up lines,
// where the id is a *combat* skill (Dagger, Evasion) carried in the message's own data1.
// vendor/server/src/map/enums/msg_basic.h:65,74,176.
const COMBAT_SKILL_MESSAGES: &[u16] = &[38, 53, 310];

// The one `<skill>` line whose id is an ability, not a weapon skill: only ability_state and
// petskill_state emit it, both with `param = ability id`
// (vendor/server/src/map/ai/states/ability_state.cpp:137-140).
const READIES_ABILITY_MESSAGE: u16 = 326;

fn skill_name(message_num: u16, data1: u32, action_id: u32) -> String {
    if COMBAT_SKILL_MESSAGES.contains(&message_num) {
        return ffxi_vocab::skill_names::lookup(data1 as u8)
            .map(str::to_string)
            .unwrap_or_else(|| format!("skill #{data1}"));
    }
    if message_num == READIES_ABILITY_MESSAGE {
        return ability_name(action_id);
    }
    ffxi_vocab::tp_move_names::lookup(action_id as u16)
        .map(str::to_string)
        .unwrap_or_else(|| format!("skill #{action_id}"))
}

fn ability_name(action_id: u32) -> String {
    ffxi_vocab::ability_names::lookup(action_id as u16)
        .map(str::to_string)
        .unwrap_or_else(|| format!("ability #{action_id}"))
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
        spans: Vec::new(),
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
            .and_then(ffxi_vocab::emote_names::lookup)
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
    let resolved_action_id = action_id.unwrap_or(data1);
    if s.contains("<skill>") {
        s = s.replace(
            "<skill>",
            &skill_name(message_num, data1, resolved_action_id),
        );
    }

    if s.contains("<spell>") {
        let name = ffxi_vocab::spell_names::lookup(resolved_action_id as u16)
            .map(str::to_string)
            .unwrap_or_else(|| format!("spell #{resolved_action_id}"));
        s = s.replace("<spell>", &name);
    }
    if s.contains("<ability>") {
        s = s.replace("<ability>", &ability_name(resolved_action_id));
    }
    if s.contains("<item>") {
        let name = ffxi_vocab::item_names::lookup(resolved_action_id as u16)
            .map(str::to_string)
            .unwrap_or_else(|| format!("item #{resolved_action_id}"));
        s = s.replace("<item>", &name);
    }
    if s.contains("<job>") {
        let name = ffxi_vocab::job_names::lookup(resolved_action_id as u16)
            .map(str::to_string)
            .unwrap_or_else(|| format!("job #{resolved_action_id}"));
        s = s.replace("<job>", &name);
    }
    if s.contains("<status>") {
        let name = ffxi_vocab::status_names::lookup(data1 as u16)
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
        text_entry: false,
        grid: None,
        custom_menu: false,
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
        text_entry: false,
        grid: None,
        custom_menu: false,
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
        text_entry: false,
        grid: None,
        custom_menu: false,
    })
}

/// An [`EventTrigger`] from an event-trigger packet (0x32/0x33/0x34), reusing the
/// raw decoders. `npc_name` is left for the caller, which owns the name caches.
///
/// The event id the client runs is `EventPara` (`dialog.event_para`), NOT
/// `EventNum` — LSB sets `EventNum = PChar->getZone()` and `EventPara =
/// eventInfo->eventId` (vendor/server/src/map/packets/s2c/0x032_event.cpp,
/// 0x034_eventnum.cpp). The same `EventPara` is what the server validates on the
/// 0x05B EVENT_END (`isInEvent(EventPara)`).
///
/// `EventNum2` is the string table. 0x032 sets it to the char's zone and 0x033
/// has no such field at all (our decoder leaves it 0), so only 0x034 can
/// redirect it; a zero there means "same zone as the script".
fn event_trigger(sub: &framing::SubPacket<'_>) -> Option<EventTrigger> {
    use ffxi_proto::map::s2c;
    let dialog = match sub.opcode {
        s2c::EVENT => decode_event_0x032(sub.data)?,
        s2c::EVENTSTR => decode_event_0x033(sub.data)?,
        s2c::EVENTNUM => decode_event_0x034(sub.data)?,
        _ => return None,
    };
    Some(EventTrigger {
        event_zone: dialog.event_num,
        text_zone: if dialog.event_num2 != 0 {
            dialog.event_num2
        } else {
            dialog.event_num
        },
        unique_no: dialog.npc_id,
        act_index: dialog.act_index,
        event_id: dialog.event_para,
        params: dialog.nums,
        npc_name: None,
    })
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

use ffxi_vocab::vana_time::VANA_EPOCH_UNIX;

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
    if remaining == 0 || remaining > kuluu_snapshot::MAX_STATUS_TIMER_SECS {
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
    let now_unix = kuluu_snapshot::recast_now_unix();
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
        out.push((timer_id, now_unix.saturating_add(timer as u32)));
    }
    out
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
            spans: Vec::new(),
            channel: ChatChannel::Say,
            sender: dialog.npc_name.clone().unwrap_or_default(),
            text: text.clone(),
            server_ts: 0,
        },
    });
}

/// Mode low byte of s2c 0x052 EVENTUCOFF (u32 right after the 4-byte
/// sub-header); CancelEvent packs the cancelled event id in the high bits
/// (vendor/server/src/map/packets/s2c/0x052_eventucoff.cpp:30-34).
fn eventucoff_mode_of(data: &[u8]) -> Option<u32> {
    let bytes: [u8; 4] = data.get(0..4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes) & ffxi_proto::map::eventucoff_mode::MODE_MASK)
}

/// s2c 0x052 EVENTUCOFF releases the client from an event user-control lock
/// (vendor/server/src/map/packets/s2c/0x052_eventucoff.h:26-33). CancelEvent
/// arrives only after the server already dropped the event (release()/skipEvent
/// call endCurrentEvent — vendor/server/src/map/lua/lua_baseentity.cpp:1374-1385,
/// vendor/server/src/map/entities/charentity.cpp:3319-3336), so no 0x05B goes
/// back; the local event state is dropped instead. Fishing release = a rejected
/// cast (no rod / bait / fishing spot) or the end of fishing. EventRecvPending —
/// the ack after every processed 0x05B (0x05b_eventend.cpp:71) — must NOT clear
/// anything: a chained event's 0x032 trigger can precede it.
fn handle_eventucoff(
    data: &[u8],
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    cutscene: &mut crate::event_dialog::CutsceneScope,
    event_tx: &broadcast::Sender<AgentEvent>,
) {
    match eventucoff_mode_of(data) {
        Some(ffxi_proto::map::eventucoff_mode::FISHING) => {
            let _ = event_tx.send(AgentEvent::FishingEnded);
        }
        Some(ffxi_proto::map::eventucoff_mode::CANCEL_EVENT) => {
            pending_event_end.clear();
            cutscene.end(crate::event_dialog::EventSessionExit::Cancelled, event_tx);
            let _ = event_tx.send(AgentEvent::EventEnded);
        }
        _ => {}
    }
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
    // "NS" chat kinds mean "No speaker object displayed" — retail renders the
    // text with no speaker attribution (used for unattributed NPC narration like
    // the Home Point description). Blank the sender so the HUD omits the prefix
    // (vendor/server/src/map/enums/chat_message_type.h:38-41,53).
    let sender = if is_no_speaker_chat_kind(kind) {
        String::new()
    } else {
        trim_nul_string(&data[4..PREFIX])
    };
    let text = decode_chat_text(&data[PREFIX..]);
    Some(ChatLine {
        spans: Vec::new(),
        channel: ChatChannel::from_chat_kind(kind),
        sender,
        text,
        server_ts: 0,
    })
}

/// The `MESSAGE_NS_*` "No speaker object displayed" chat kinds
/// (chat_message_type.h): same channel/color as their base kind but with no
/// speaker attribution.
fn is_no_speaker_chat_kind(kind: u8) -> bool {
    use ffxi_proto::map::chat_kind as k;
    matches!(
        kind,
        k::NS_SAY | k::NS_SHOUT | k::NS_PARTY | k::NS_LINKSHELL | k::NS_LINKSHELL2
    )
}

// Server customMenu prompt (home point Set/Yes/No, quest confirmations, …):
// GP_SERV_COMMAND_CHAT_STD with type MESSAGE_GMPROMPT and sender name
// `_CUSTOM_MENU`, message = quoted-concat `"Title""Opt1""Opt2"…`
// (vendor/server/src/map/lua/lua_baseentity.cpp:615 customMenu +
// luautils.cpp:5288 SetCustomMenuContext). The reply round-trips as a
// `_CUSTOM_MENU` tell the server routes to HandleCustomMenu
// (0x0b6_chat_name.cpp:79-:82).
const CUSTOM_MENU_SENDER: &str = "_CUSTOM_MENU";
const MESSAGE_GMPROMPT: u8 = 12; // vendor/server/src/map/enums/chat_message_type.h:37
                                 // HandleCustomMenu (luautils.cpp:5323) extracts the result after this marker and
                                 // drops the trailing `)`; the "Canceled." payload takes the onCancelled branch
                                 // (luautils.cpp:5368 NA).
const CUSTOM_MENU_RESULT_MARKER: &str = ": Result (";
const CUSTOM_MENU_CANCEL: &str = "Canceled.";

/// Decode a customMenu prompt from a chat-std body, returning `(title, options)`.
/// `None` for any non-customMenu chat so the caller falls back to a plain line.
fn decode_custom_menu(data: &[u8]) -> Option<(String, Vec<String>)> {
    const PREFIX: usize = 4 + 15;
    if data.len() < PREFIX || data[0] != MESSAGE_GMPROMPT {
        return None;
    }
    if trim_nul_string(&data[4..PREFIX]) != CUSTOM_MENU_SENDER {
        return None;
    }
    let text = decode_chat_text(&data[PREFIX..]);
    let mut parts = parse_quoted_concat(&text).into_iter();
    let title = parts.next()?;
    Some((title, parts.collect()))
}

/// Split the server's `"a""b""c"` quoted-concatenation into its segments.
fn parse_quoted_concat(s: &str) -> Vec<String> {
    s.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The `_CUSTOM_MENU` tell body the server's HandleCustomMenu parser expects:
/// retail's `GMTELL(name): Question(title): Result (option)`. The option (or the
/// "Canceled." sentinel) must be the final token so the server's trailing-`)`
/// strip recovers it exactly.
fn custom_menu_reply(player: &str, title: &str, option: Option<&str>) -> String {
    let result = option.unwrap_or(CUSTOM_MENU_CANCEL);
    format!("GMTELL({player}): Question({title}){CUSTOM_MENU_RESULT_MARKER}{result})")
}

fn decode_chat_text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    ffxi_proto::autotranslate::decode(&bytes[..end])
}

fn trim_nul_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// GP_CLI_COMMAND_AUC::validate ranges Commission/BidPrice/LimitPrice
/// 1..=AUCTION_PRICE_MAX (vendor/server/src/map/packets/c2s/0x04e_auc.cpp).
fn auc_price_valid(price: u32, event_tx: &broadcast::Sender<AgentEvent>) -> bool {
    let valid = (1..=ffxi_proto::decode::AUCTION_PRICE_MAX).contains(&price);
    if !valid {
        let _ = event_tx.send(AgentEvent::Error {
            message: format!(
                "auction price {price} outside 1..={}",
                ffxi_proto::decode::AUCTION_PRICE_MAX
            ),
        });
    }
    valid
}

#[derive(Debug)]
enum AhSearchQuery {
    Browse { category: u8, sorts: Vec<u32> },
    History { item_id: u16, stack: bool },
}

/// Runs one AH search-server round-trip off the session loop so the 200ms
/// reactor tick never blocks on TCP. `inflight` serializes searches; the task
/// clears it before publishing so the result event is the reopen signal.
fn spawn_ah_search(
    inflight: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    event_tx: &broadcast::Sender<AgentEvent>,
    host: String,
    query: AhSearchQuery,
) {
    use std::sync::atomic::Ordering;
    if inflight.swap(true, Ordering::AcqRel) {
        let _ = event_tx.send(AgentEvent::Error {
            message: "auction search already in flight".into(),
        });
        return;
    }
    let _ = event_tx.send(AgentEvent::AuctionOpStarted {
        op: crate::state::AuctionBusy::Downloading,
    });
    let inflight = inflight.clone();
    let event_tx = event_tx.clone();
    tokio::spawn(async move {
        let event = match query {
            AhSearchQuery::Browse { category, sorts } => {
                match crate::search_client::ah_list(&host, category, &sorts).await {
                    Ok(catalog) => AgentEvent::AuctionBrowseResults {
                        category,
                        total: catalog.total,
                        listings: catalog.listings.into_iter().map(Into::into).collect(),
                    },
                    Err(e) => AgentEvent::AuctionSearchFailed {
                        message: format!("AH browse: {e:#}"),
                    },
                }
            }
            AhSearchQuery::History { item_id, stack } => {
                match crate::search_client::ah_history(&host, item_id, stack).await {
                    Ok(h) => AgentEvent::AuctionHistoryResults {
                        history: crate::state::AhHistoryView::from_wire(h, stack),
                    },
                    Err(e) => AgentEvent::AuctionSearchFailed {
                        message: format!("AH history: {e:#}"),
                    },
                }
            }
        };
        inflight.store(false, Ordering::Release);
        let _ = event_tx.send(event);
    });
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
                spans: Vec::new(),
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
                spans: Vec::new(),
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
        // GROUP_ATTR carries no GAttr; NO_PARTY keeps an attrs-only update from
        // claiming party 0 until the next GROUP_LIST refreshes it.
        party_no: extra.map(|e| e.party_no).unwrap_or(decode::NO_PARTY),
        in_mog_house: attrs.moghouse_flg != 0,
    }
}

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

// Head-look targid we broadcast: a target-bearing command aimed at ourselves (or
// no target) reads as "looking at nothing", which retail encodes as facetarget 0.
fn face_target_for(target_index: u16, self_act_index: Option<u16>) -> u16 {
    if target_index == 0 || Some(target_index) == self_act_index {
        0
    } else {
        target_index
    }
}

/// Also the resolution a cutscene's timed waits are served at, so it bounds how
/// far a fade can overrun its authored duration.
const SESSION_TICK_PERIOD: std::time::Duration = std::time::Duration::from_millis(100);

const MOVE_EMISSION_PERIOD: std::time::Duration = std::time::Duration::from_millis(100);

const MOVE_BIG_JUMP_YALMS: f32 = 0.5;

/// How long after a zone-in that out-of-order / duplicate self-position carriers
/// from around the transition are still expected to arrive (the double-bootstrap
/// re-floods, and pre-transition echoes can land late). Within this window a carrier
/// whose position is far (> snap threshold) from where we actually stand is treated as
/// stale — an old-zone coordinate or re-sent snapshot — and must not yank us across
/// zones (see reconcile_self_pos `refuse_snap`). A legitimate server teleport never
/// happens in the first moments of a zone, so refusing Snap here cannot mask a real
/// correction.
const ZONE_IN_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq)]
enum SelfPosReconcile {
    KeepLocal,

    Rubberband { target: Vec3 },

    Snap,
}

fn reconcile_self_pos(local: Vec3, server: Vec3, refuse_snap: bool) -> SelfPosReconcile {
    let dx = server.x - local.x;
    let dy = server.y - local.y;
    let dz = server.z - local.z;
    let dist_sq = dx * dx + dy * dy + dz * dz;

    if dist_sq <= 2.0 * 2.0 {
        SelfPosReconcile::KeepLocal
    } else if dist_sq <= 10.0 * 10.0 {
        SelfPosReconcile::Rubberband { target: server }
    } else if refuse_snap {
        // Post-zone-in settle window: a far (>snap) self-position carrier is an
        // out-of-order/duplicate position from around the transition (an old-zone
        // coordinate or re-sent pre-transition snapshot), not a real teleport —
        // keep our local seed instead of snapping across zones. A legitimate server
        // teleport never lands in the first moments of a zone, so this cannot mask a
        // genuine correction.
        SelfPosReconcile::KeepLocal
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

/// Keepalive-tick state the pending-EVENT_END release reads. Named fields, not
/// positional `bool`s: a swapped pair here silently inverts the release policy.
struct EventEndFlushInputs {
    user_driven: bool,
    watchdog_fires: bool,
    walked_away: bool,
}

/// What the tick owes the rest of the loop once the pinned events are drained.
struct EventEndFlush {
    payload: Vec<u8>,
    next_sub_seq: u16,
    released: usize,
    clear_dialog: bool,
}

/// Drain the events still owed a 0x05B and build their subpackets, or `None`
/// when this tick must leave them pinned.
///
/// Retail never times a dialog out, so the grace timer exists only to unpin an
/// event with nothing on screen; with a frame the player is still reading, the
/// escape hatches are walking away and /endevent.
///
/// The drained 0x05B ends the event server-side, so only the walk-away path —
/// where the player abandoned the frame — clears the VM session that owns it.
/// Every other path (notably `!user_driven`: agent_io, agent_socket, kuluu-mcp,
/// headless) keeps the session alive so the consumer can still walk the dialog
/// tree locally; [`take_pending_event_end`] is what stops that walk from
/// sending a second EVENT_END.
fn flush_pending_event_end(
    inputs: EventEndFlushInputs,
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    active_dialog: Option<(u32, u16, u16)>,
    event_zone: u16,
    sub_seq: u16,
) -> Option<EventEndFlush> {
    let dialog_open = active_dialog.is_some();
    let flush =
        !inputs.user_driven || inputs.walked_away || (inputs.watchdog_fires && !dialog_open);
    if !flush || pending_event_end.is_empty() {
        return None;
    }
    let mut out = EventEndFlush {
        payload: Vec::new(),
        next_sub_seq: sub_seq,
        released: 0,
        clear_dialog: false,
    };
    for pinned in pending_event_end.drain(..) {
        let (unique_no, act_index, event_id) = pinned;
        out.payload.extend(build_subpacket_event_end(
            out.next_sub_seq,
            unique_no,
            act_index,
            event_zone,
            event_id,
            0,
        ));
        out.next_sub_seq = out.next_sub_seq.wrapping_add(1);
        out.released += 1;
        out.clear_dialog |= inputs.walked_away && active_dialog == Some(pinned);
    }
    Some(out)
}

/// `pending_event_end` is the set of events the server still owes us a 0x05B
/// for, so taking the entry is the authorization to send one. A VM session that
/// outlived the keepalive auto-release must end locally without resending: LSB
/// drops an EVENT_END whose EventPara no longer matches `currentEvent`
/// (vendor/server/src/map/packets/c2s/validation.cpp:58-77).
fn take_pending_event_end(
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    unique_no: u32,
    event_id: u16,
) -> bool {
    let before = pending_event_end.len();
    pending_event_end.retain(|(uid, _, en)| !(*uid == unique_no && *en == event_id));
    before != pending_event_end.len()
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
        name_vis: 0,
        claim_id: 0,
        speed: 0,
        speed_base: 0,
        look: None,
        npc_state: None,
        char_flags: None,
        status: 0,
        mount_id: None,
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
mod tests;
