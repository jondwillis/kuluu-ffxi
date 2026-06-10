//! Session actor — orchestrates auth → lobby → map bootstrap → zone-in →
//! keepalive, and emits typed `AgentEvent`s for both the JSON sidechannel and
//! (eventually) the TUI to subscribe to.
//!
//! Sequence-number bookkeeping for the map session lives here. The server
//! caches the last bundle it sent and *resends it* if our `sync_in` doesn't
//! match its `server_packet_id` (failure mode #1 in the plan: silent
//! sequence desync). Tracking `server_last_seq` from incoming bundles and
//! using it as the ack on outgoing bundles keeps us walking forward.

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

/// Resolves static-NPC names from a per-zone DAT file, lazily loading
/// the table on first miss and swapping it out when the entity's
/// zone bits differ from the current cached table.
///
/// Holds at most one zone's table at a time (~30 KB). The wire path
/// only sees CHAR_NPC for the player's current zone, so a single-slot
/// cache covers the common case without growing across long sessions.
/// Returns `None` when no DAT install is configured (soft-degrade) or
/// when the id doesn't belong to any reachable zone DAT.
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

    /// Look up the display name for a static NPC by its wire id.
    /// Returns `None` for dynamic-entity ids (trusts/pets/fellows —
    /// their `targid + 0x100` puts their slot above the DAT range) and
    /// for unconfigured DAT roots.
    fn lookup(&mut self, npc_id: u32) -> Option<&str> {
        let root = self.root.as_ref()?;
        let (zone, _slot) = ffxi_dat::split_id(npc_id)?;
        // Swap the active table when the entity's zone changes.
        let zone_matches = self.current.as_ref().is_some_and(|t| t.zone_id() == zone);
        if !zone_matches {
            self.current = match ffxi_dat::NpcNameTable::open(root, zone) {
                Ok(table) => Some(table),
                Err(err) => {
                    // Common — many zone ids have no NPC list DAT (battlefields,
                    // unfinished zones). Log at debug so a populated zone with
                    // a real install doesn't get spammed at info.
                    tracing::debug!(zone, error = %err, "no NPC-name DAT for zone");
                    None
                }
            };
        }
        self.current.as_ref()?.lookup_by_id(npc_id)
    }
}

/// Which method to use for character selection.
/// Exactly one of `char_id` or `char_name` is allowed — the type system enforces it.
#[derive(Clone, Debug)]
pub enum CharSelection {
    Id(u32),
    Name(String),
}

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
    pub char_selection: CharSelection,
    /// Pre-prepared auth + lobby state. When `Some(...)`, `run` skips its
    /// own `auth.login` *and* `lobby.handshake` steps and goes straight to
    /// the map bootstrap with the supplied handoff/key. Required for the
    /// launcher path: the server's `session_t` is keyed on
    /// `(ip, session_hash)` and tracks `data_session` / `view_session`
    /// shared_ptrs — re-handshaking with the same hash routes responses
    /// to stale (closed) sockets and produces silent EOFs.
    /// MCP / `Play` callers that don't pre-resolve set this to `None`.
    pub initial_state: Option<InitialState>,
    /// When `true`, the keepalive tick will *not* auto-flush
    /// `pending_event_end` — the operator (or the dialog HUD) is expected
    /// to issue `AgentCommand::EndEvent` explicitly. The native viewer
    /// sets this to `true` so the dialog panel stays visible long enough
    /// to read; headless MCP/agent callers leave it at `false` so
    /// unattended sessions don't get stuck in events.
    pub user_driven_events: bool,
    /// Optional FFXI client DAT root. When `Some`, CHAR_NPC name
    /// resolution falls back to the per-zone NPC-name DAT
    /// (`file_id = 6720 + zone`) when the wire packet didn't carry a
    /// name. `Arc` because the same `DatRoot` is shared across all
    /// reconnects within a `run()` invocation and across cloned Configs.
    pub dat_root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,
}

/// Caller-supplied lobby completion. All three fields are required
/// together: the bootstrap needs `auth.session_hash` for the ticket,
/// `handoff.server_ip/server_port` for the map endpoint, and `key3` is
/// the blowfish seed the map server reads back from `accounts_sessions`.
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

    // Phase 1 — auth + lobby. Skipped entirely when the caller has already
    // run them (launcher path); needed for MCP / `Play` which only get
    // raw credentials and a charid.
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

    // Phase 2 — map session, with reconnect-on-zone-change as the outer loop.
    // The MapClient (and its UDP socket) is constructed *once* and lives
    // across all zone-change reconnects. See `MapClient::retarget` for why
    // the socket must persist on a single-process LSB dev stack.
    let mut current_seed = key3;
    let mut iteration: u32 = 0;
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
                // Re-apply `map_host_override` on reconnect. LSB's
                // `0x00B Iwasaki` field reports the new map server's
                // *internal* address — for a containerized dev stack
                // (Docker / Colima) that's the container-network IP,
                // unreachable from the host. The override translates
                // this on the initial connect at line 146; the same
                // translation must apply per-zone-change. Only the port
                // varies between zones in dev (single map process per
                // container), so we keep the override host and adopt
                // LSB's reported port.
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
                // Retarget the existing socket — do NOT rebind. See
                // `MapClient::retarget` for the LSB session-lookup
                // rationale (match-by-(ip,port) requires source-port
                // continuity).
                map.retarget(target, current_seed);
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
    cfg: &Config,
    _auth_session: &crate::auth_client::AuthSession,
    bootstrap: &BootstrapArgs<'_>,
    map: &mut MapClient,
    cert_sha256: Option<String>,
    iteration: u32,
    cmd_rx: &mut mpsc::Receiver<AgentCommand>,
    event_tx: &broadcast::Sender<AgentEvent>,
) -> Result<MapOutcome> {
    // MapClient lifetime (including its UDP socket) is owned by the
    // outer `run` so it spans reconnects; the initial Stage::MapBootstrap
    // emit + 1500ms ZMQ pacing happen there too. This function just
    // sends bootstraps and runs one map session.

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
    let mut name_cache: std::collections::HashMap<u32, String> = Default::default();
    // Static-NPC name resolver backed by the FFXI client install DATs.
    // Lazy-loads one zone's name table on first lookup miss; swaps it
    // out when the next CHAR_NPC's zone bits differ. `cfg.dat_root`
    // is `None` when the install couldn't be found at boot.
    let mut npc_name_resolver = NpcNameResolver::new(cfg.dat_root.clone());
    // Rate-limit map for `NameExtractionMiss` emission. Keyed by
    // `(entity_id, miss_kind)` so a single entity can emit multiple kinds
    // of miss (e.g., NameBitClear during regular ticks plus a one-off
    // NameBitSetExtractionFailed) without one stomping the other.
    let mut name_miss_dedup: std::collections::HashMap<
        (u32, crate::state::NameMissKind),
        std::time::Instant,
    > = Default::default();
    let mut current_zone_id: u16 = 0;
    // Synced from CHAR_PC for self during the flood-drain — needed
    // so the first keepalive after Stage::InZone sends server-authoritative
    // coords rather than `Position::default()`.
    let mut self_pos = Position::default();
    // False until a `0x0DF GROUP_ATTR` for self lands with `MoghouseFlg!=0`.
    // Tracked across the flood drain *and* re-used after the wait-loop hands
    // off to the per-tick reactor (see the outer `flood_in_mog_house`).
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
                        &mut name_miss_dedup,
                        &mut current_zone_id,
                        &mut self_pos,
                        &mut npc_name_resolver,
                        &mut flood_in_mog_house,
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
        name_miss_dedup,
        self_pos,
        npc_name_resolver,
    )
    .await
}

/// Classify a CHAR_NPC (0x00E) entity from its decoded `look.size` plus the
/// per-packet markers the caller reads out of the entity-update body. Split
/// out of [`handle_sub_packet`] so the LSB-boundary invariants are unit-
/// testable.
///
/// The hard case is `look.size` 0/5/6 (standard monster meshes): real mobs
/// AND a class of static/triggerable NPCs that reuse a monster model (e.g.
/// the Auction House counter) both land here. We resolve it with two LSB
/// signals, in priority order:
///   * `act_index` (targid) in 0x700..=0x8FF → a dynamically-spawned
///     MOB/PET/TRUST (`zone_entities.cpp:594-627`). This is on *every* packet
///     and is authoritative, so a field mob classifies as `Mob` even when an
///     individual update omits the live-mob marker (the marker is gated on
///     `UPDATE_HP`, which made intermittently-updating mobs flip to `Npc`).
///   * otherwise (static targid `& 0xFFF`, < 0x700 — static NPCs and static
///     instance/BCNM mobs) fall back to the live-mob marker (body byte 0x25 =
///     `hp>0 ? 0x08 : 0`), which LSB writes only in the TYPE_MOB/PET/TRUST
///     branch of `entity_update.cpp`; the TYPE_NPC branch never touches it.
///     That byte is gated on `UPDATE_HP`, so:
///       - no UPDATE_HP this packet → can't tell → `Other`, letting
///         `state::merge_kind` keep any prior specialized kind;
///       - UPDATE_HP + marker set → live creature → `Mob`;
///       - UPDATE_HP + marker clear → static NPC on a monster model → `Npc`.
///
/// A PC-owned standard model (`PMaster` is a PC) is `Pet` regardless.
fn classify_char_npc(
    look_size: Option<u16>,
    act_index: u16,
    owned_by_pc: bool,
    has_hp_update: bool,
    is_live_mob: bool,
) -> EntityKind {
    // LSB allocates dynamic entities (MOB/PET/TRUST) a targid in
    // 0x700..=0x8FF (`zone_entities.cpp:595-627`, range constant
    // `DYNAMIC_ENTITY_TARGID_RANGE_START = 0x700`), and documents at
    // `zone_entities.cpp:594` that 0x0E updates are valid for "0 to 1023 and
    // 1792 to 2303" (0x700..=0x8FF). A standard-mesh CHAR_NPC in that range
    // is therefore a genuine spawned creature — present on *every* packet,
    // unlike the UPDATE_HP-gated live-mob marker. Static NPCs and static
    // instance mobs get `targid & 0xFFF` (< 0x700) and still need the marker
    // to disambiguate (the AH counter reuses a monster mesh but is an NPC).
    let dynamic_targid = (0x700..=0x8FF).contains(&act_index);
    match look_size {
        Some(0) | Some(5) | Some(6) => {
            if owned_by_pc {
                EntityKind::Pet
            } else if dynamic_targid {
                // Authoritative: a live creature regardless of whether this
                // packet carried the (flaky, UPDATE_HP-gated) marker. Fixes
                // dynamic mobs that intermittently classified as Npc.
                EntityKind::Mob
            } else if !has_hp_update {
                EntityKind::Other
            } else if is_live_mob {
                EntityKind::Mob
            } else {
                EntityKind::Npc
            }
        }
        Some(1) | Some(7) => EntityKind::Npc,
        Some(2) | Some(3) | Some(4) => EntityKind::Other,
        // Truncated body or unknown look size — emit `Other` so
        // `state::merge_kind` preserves any prior specialized classification.
        _ => EntityKind::Other,
    }
}

/// Decode a single S2C sub-packet and emit typed `AgentEvent`s. Returns the
/// `(UniqueNo, ActIndex, EventNum)` triple if it's an event-start packet so
/// the caller can queue an auto-dismiss `0x05B EVENT_END`.
///
/// `self_char_id` lets us recognize the player's own CHAR_PC packet during
/// the zone-in flood and stash the player's per-zone `ActIndex` in
/// `self_act_index` — required when sending packets that target the player
/// (e.g. `0x05E` MAPRECT for zone-line transitions).
///
/// `name_cache` is a small id→name table accumulated from CHAR_PC/CHAR_NPC
/// packets. The 0x029/0x02D battle-message decoders look names up here to
/// substitute `<user>`/`<target>` placeholders without re-reading the
/// snapshot. It's a session-loop-local mirror of the entity name set; a
/// miss falls back to a hex id rather than failing.
fn handle_sub_packet(
    sub: &framing::SubPacket<'_>,
    event_tx: &broadcast::Sender<AgentEvent>,
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    self_char_id: u32,
    // The locally-authenticated character name from the lobby handshake.
    // LSB only sends the user's PC name in CHAR_PC packets when
    // `SendFlg.Name` is set (typically once per spawn), and zoning-in
    // floods don't always include it — so we seed the self-entity from
    // this known-good value on `0x00A LOGIN` instead of waiting for an
    // optional later CHAR_PC.
    self_char_name: &str,
    self_act_index: &mut Option<u16>,
    name_cache: &mut std::collections::HashMap<u32, String>,
    // Per-(entity, miss-kind) timestamps used to rate-limit
    // `NameExtractionMiss` events. Without this, a populated zone where
    // most entities never sent UPDATE_NAME would emit hundreds of
    // identical misses per second across the attach socket.
    name_miss_dedup: &mut std::collections::HashMap<
        (u32, crate::state::NameMissKind),
        std::time::Instant,
    >,
    // Captures the current zone id as it's decoded from the 0x00A LOGIN
    // sub-packet. The caller's mirror of this value is used by the
    // `Snapshot` handler to re-emit a fresh `Connected{zone_id}` for
    // attach-mode resync — without it, a late-attaching MCP can't tell
    // what zone the running session is in.
    current_zone_id: &mut u16,
    // Synced from inbound `CHAR_PC` for self so the keepalive's
    // `0x015 POS` carries the server's authoritative spawn coords on
    // zone-in. Without this sync, the loop's local `self_pos` stays at
    // the previous-zone value (or `Position::default()`) and the next
    // keepalive overrides the server's `loc.p` with garbage — lands
    // the player at the origin or wherever Move last walked us.
    self_pos: &mut Position,
    // Static-NPC name resolver backed by the FFXI client install
    // DATs. Used as a fallback for CHAR_NPC packets that arrive
    // without `UPDATE_NAME` set — LSB's `entity_update.cpp:293-295`
    // strips that bit for equipped-model spawns, which is most
    // ambient NPCs. Dynamic entities (trusts/pets/fellows) keep
    // arriving with names via 0x67/0x68 and don't use this path.
    npc_name_resolver: &mut NpcNameResolver,
    // Tracks whether the most recent self `MoghouseFlg` was set. The session
    // loop uses this to emit a single chat-system hint on the false→true
    // edge so the operator knows why their entity list went empty (LSB
    // keeps zone id equal to the surrounding city while in a Mog House —
    // there's no other wire signal).
    was_in_mog_house: &mut bool,
) {
    use ffxi_proto::map::s2c;
    match sub.opcode {
        op if op == s2c::LOGIN => {
            // 0x00A carries the zone-in payload — `ZoneNo` at body[44..48]
            // and the server's authoritative spawn `GP_SERV_POS_HEAD` at
            // body[0..44]. Without decoding the PosHead, `self_pos` (and
            // the renderer's anchor) defaults to `Position::default()`
            // until a CHAR_PC for self happens to arrive — which is why
            // the first keepalive after zone-in used to send (0, 0, 0)
            // and the camera landed in the wrong place. Seed both the
            // session-loop's local `self_pos` AND the entity list (via
            // `EntityUpserted`) so the snapshot's derived `self_pos`
            // reflects truth from the very first packet of zone-in.
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
                // ZoneChanged first — its `apply_event` clears the entity
                // list (entities from the old zone are stale). Emitting the
                // self-seed upsert *before* this would just get wiped.
                let _ = event_tx.send(AgentEvent::ZoneChanged {
                    from: None,
                    to: login.zone_no,
                });
                // 0x00A `GameTime` is the server's Earth-seconds-since-
                // vanadiel_epoch counter. Surface it so the viewer's
                // VanaClock can anchor its sun/moon and HUD clock to
                // server truth instead of `SystemTime::now()`.
                if let Some(game_time) = login.game_time {
                    let _ = event_tx.send(AgentEvent::VanaTimeSynced { game_time });
                }
                // LSB embeds `MusicNum[5]` directly in the LOGIN body
                // for the zone's pre-set slots (Day/Night/CombatSolo/
                // CombatParty/Mount). Out-of-band 0x05F arrives only
                // for runtime `changeMusic()` calls. Without
                // surfacing these here, the viewer hears silence
                // until something in a Lua script touches the music
                // — which never happens in most zones.
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
                    *self_pos = Position {
                        pos: Vec3 {
                            x: head.x,
                            y: head.y,
                            z: head.z,
                        },
                        heading: head.dir,
                        speed: head.speed,
                        speed_base: head.speed_base,
                    };
                    // Seed the self entity from LOGIN's `PosHead` so the
                    // entity list (and therefore the wire snapshot's
                    // `self_pos`, which is now derived from it) reflects
                    // server-authoritative spawn coords from the very
                    // first packet of zone-in. Without this, callers fall
                    // back to `Position::default()` (origin) until a
                    // CHAR_PC for self happens to arrive.
                    let _ = event_tx.send(AgentEvent::EntityUpserted {
                        entity: Entity {
                            id: head.unique_no,
                            act_index: head.act_index,
                            kind: EntityKind::Pc,
                            // Seed the self-entity name from the lobby
                            // handshake. CHAR_PC for self only carries the
                            // name when SendFlg.Name is set, and that's
                            // not guaranteed on every zone-in flood — so
                            // without this seed the tab-target list shows
                            // the player as "?" until/unless a name-bearing
                            // CHAR_PC happens to arrive.
                            name: Some(self_char_name.to_string()),
                            pos: Vec3 {
                                x: head.x,
                                y: head.y,
                                z: head.z,
                            },
                            heading: head.dir,
                            hp_pct: Some(head.hpp),
                            bt_target_id: head.bt_target_id,
                            claim_id: 0,
                            speed: head.speed,
                            speed_base: head.speed_base,
                            // LOGIN packet doesn't carry a look block for
                            // self — CHAR_PC fills this in later when
                            // SendFlg.Name happens to be set.
                            look: None,
                        },
                    });
                    // Mirror to legacy `state.self_pos` so the fallback
                    // path in `state_to_snapshot` (used pre-`char_id`
                    // resolution) also has the right value.
                    let _ = event_tx.send(AgentEvent::PositionChanged { pos: *self_pos });
                }
            }
        }
        op if op == s2c::CHAR_PC || op == s2c::CHAR_NPC => {
            if let Ok(head) = decode::PosHead::decode(sub.data) {
                let kind = if op == s2c::CHAR_PC {
                    EntityKind::Pc
                } else {
                    // CHAR_NPC (0x00E) is LSB's catch-all for every
                    // non-PC entity class. We discriminate via the
                    // `look.size` field LSB writes at packet offset
                    // 0x30 (= body[0x2C..0x2C+2]) — the MODELID_TYPE
                    // enum from `vendor/server/.../entity_update.h:30`:
                    //
                    //   0 STANDARD / 5 UNK_5 / 6 AUTOMATON
                    //       — raw monster meshes (mobs, PUP pets,
                    //         charmed beasts).
                    //   1 EQUIPPED / 7 CHOCOBO
                    //       — equipped character (every friendly NPC
                    //         in town, mounts, fellows).
                    //   2 DOOR / 3 ELEVATOR / 4 SHIP
                    //       — static furniture / transport.
                    //
                    // `look.size` is written on *every* CHAR_NPC packet
                    // (`entity_update.cpp:451-484`, outside the
                    // UPDATE_HP gate), so we always have it to classify.
                    // But look.size 0/5/6 (standard monster meshes) is
                    // NOT mob-exclusive: a class of static/triggerable
                    // NPCs reuse a monster model (e.g. the Auction House
                    // counter), so look.size alone tags them as Mob and
                    // makes "attack" wrongly available. We disambiguate
                    // standard-model entities with LSB's live-mob marker
                    // (see `classify_char_npc`).
                    //
                    // Why not the allegiance byte at packet 0x29:
                    // `CNpcEntity` defaults `allegiance = ALLEGIANCE_TYPE::MOB`
                    // (`npcentity.cpp:44`), so most friendly NPCs come
                    // through indistinguishable from mobs.
                    //
                    // Pet split: byte 0x27 bit 0x08 = "PMaster is a PC"
                    // (`entity_update.cpp:390-392`), gated on UPDATE_HP.
                    // Promotes a creature-shaped model with a PC owner
                    // (jug pet, BST jug, charmed mob, fellow) to Pet so
                    // the nameplate module picks the friendly palette.
                    // When UPDATE_HP isn't set we can't read it; the
                    // classifier returns Other and merge_kind preserves
                    // the prior specialized kind.
                    //
                    // Packet-offset note: the client's `sub.data` is the
                    // LSB packet body minus its 4-byte sub-header, so each
                    // client offset is the LSB offset minus 4 (verified by
                    // look.size: LSB 0x30 → 0x2C; pet bit: LSB 0x27 → 0x23).
                    const LOOK_SIZE_OFFSET: usize = 0x2C; // LSB 0x30
                    const UPDATEMASK_OFFSET: usize = 0x06; // LSB 0x0A
                    const MOB_MARKER_OFFSET: usize = 0x21; // LSB 0x25
                    const UPDATE_HP: u8 = 0x04; // baseentity.h:171
                    let look_size = sub
                        .data
                        .get(LOOK_SIZE_OFFSET..LOOK_SIZE_OFFSET + 2)
                        .map(|s| u16::from_le_bytes([s[0], s[1]]));
                    let owned_by_pc = head.send_flag & 0x04 != 0
                        && (sub.data.get(35).copied().unwrap_or(0) & 0x08) != 0;
                    let has_hp_update =
                        sub.data.get(UPDATEMASK_OFFSET).copied().unwrap_or(0) & UPDATE_HP != 0;
                    let is_live_mob =
                        sub.data.get(MOB_MARKER_OFFSET).copied().unwrap_or(0) & 0x08 != 0;
                    classify_char_npc(
                        look_size,
                        head.act_index,
                        owned_by_pc,
                        has_hp_update,
                        is_live_mob,
                    )
                };
                if op == s2c::CHAR_PC && head.unique_no == self_char_id {
                    *self_act_index = Some(head.act_index);
                    *self_pos = Position {
                        pos: Vec3 {
                            x: head.x,
                            y: head.y,
                            z: head.z,
                        },
                        heading: head.dir,
                        ..*self_pos
                    };
                }
                let wire_name = decode::PosHead::try_extract_name(op, sub.data);
                if wire_name.is_none() {
                    // Surface the miss for forensics regardless of whether
                    // the SQL-table fallback resolves below — the wire
                    // genuinely didn't carry a name this tick.
                    record_name_miss(
                        op,
                        head.unique_no,
                        head.act_index,
                        sub.data,
                        name_miss_dedup,
                        event_tx,
                    );
                }
                // Fallback: when CHAR_NPC didn't carry a name (LSB
                // overrides updatemask to 0x57 for equipped-model
                // entity spawns — entity_update.cpp:293-295 — which
                // strips UPDATE_NAME), resolve it from the FFXI client
                // DAT install. The per-zone NPC name list at
                // `file_id = 6720 + zone_id` stores the same display
                // names retail clients render; without this fallback,
                // ambient NPCs show as "?" in the target panel.
                //
                // PC names don't get this fallback — the DAT only
                // covers static (database-resident) NPCs, and the
                // self-PC name is already seeded from `self_char_name`
                // on LOGIN. Dynamic entities (trusts/pets/fellows)
                // have ids with `targid + 0x100` low bits per
                // `zone_entities.cpp:629`, putting their slot above the
                // DAT range, so the resolver naturally returns `None`
                // for them and they keep their wire-supplied name.
                let name = wire_name.or_else(|| {
                    if op == s2c::CHAR_NPC {
                        npc_name_resolver.lookup(head.unique_no).map(str::to_string)
                    } else {
                        None
                    }
                });
                // FFXI DAT files store NPC + mob names with underscores
                // standing in for spaces (`Tunnel_Worm`, `Magic_Pot`).
                // Retail clients render them with spaces; do the same
                // normalization at the wire boundary so every
                // downstream consumer (nameplate, target panel, chat
                // log, name_cache) sees `"Tunnel Worm"`. PC names
                // can't contain underscores at character-creation
                // time, so this is safe for CHAR_PC too.
                let name = name.map(|n| n.replace('_', " "));
                if let Some(n) = name.as_ref() {
                    if !n.is_empty() {
                        name_cache.insert(head.unique_no, n.clone());
                    }
                }
                // CHAR_NPC body[40..44] holds `m_OwnerID` (claim id); CHAR_PC
                // uses the same slot for `BtTargetID`. Decode them under
                // their semantic names.
                let claim_id = if op == s2c::CHAR_NPC {
                    decode::PosHead::decode_char_npc(sub.data)
                        .map(|(_, claim)| claim)
                        .unwrap_or(0)
                } else {
                    0
                };
                // Gate `hp_pct` on the UPDATE_HP bit (0x04) in the updatemask
                // byte at body[6]. LSB only writes `Hpp` at packet offset 0x1E
                // (body[26]) when UPDATE_HP is set; on position-only ticks the
                // byte is zero from the freshly-constructed packet buffer
                // (`entity_update.cpp:381-385` for mobs, `:344` for NPCs which
                // hard-code 0x64=100 under UPDATE_HP). Reporting head.hpp
                // unconditionally clobbers prior HP% with 0 / 100 noise; the
                // reducer treats `None` as "leave existing untouched."
                let send_flag = sub.data.get(6).copied().unwrap_or(0);
                let hp_pct = (send_flag & 0x04 != 0).then_some(head.hpp);
                // Look data — CHAR_NPC carries `look_t` at body[0x2C..];
                // CHAR_PC carries `GrapIDTbl[9]` at body[0x44..0x56] (race-
                // packed modelid + 8 gear slots OR'd with 0xN000 masks).
                let look = if op == s2c::CHAR_NPC {
                    decode::LookData::decode_char_npc(sub.data)
                } else if op == s2c::CHAR_PC {
                    decode::LookData::decode_char_pc(sub.data)
                } else {
                    None
                };
                // Self-CHAR_PC look-decode diagnostic: when our own
                // CHAR_PC arrives but `decode_char_pc` returns None,
                // dump the bytes around the GrapIDTbl slot. Tells us
                // whether LSB is sending a populated look block for
                // self (retail clients reconstruct appearance from
                // local equipment state, so LSB may zero the slot
                // intentionally) or whether the body is shorter than
                // the GrapIDTbl offset.
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
                        bt_target_id: head.bt_target_id,
                        claim_id,
                        speed: head.speed,
                        speed_base: head.speed_base,
                        look,
                    },
                });
            }
        }
        op if op == s2c::ENTITY_UPDATE1 => {
            // 0x067 is a multiplexed opcode: the sub-type byte at body[0]
            // selects which packet variant is on the wire. None of them
            // are position updates — the previous PosHead-based decode
            // produced phantom entities at (0, 0, 0).
            match sub.data.first().copied() {
                Some(decode::EntitySetName::SUB_TYPE) => {
                    if let Ok(ent) = decode::EntitySetName::decode(sub.data) {
                        if let Some(name) = ent.name {
                            let _ = event_tx.send(AgentEvent::EntityPatched {
                                id: Some(ent.id),
                                act_index: Some(ent.targid),
                                name: Some(name),
                                // LSB uses this packet for trusts, fellows
                                // and pankration entities — we can't tell
                                // them apart from this packet alone, so we
                                // leave `kind` to whatever the CHAR_NPC
                                // stream already established.
                                kind: None,
                                hp_pct: None,
                            });
                        }
                    }
                }
                Some(decode::CharSync::SUB_TYPE) => {
                    // PC status sync (level-sync icon, mount data, …) — not
                    // consumed by the client today. Decode for future use.
                    let _ = decode::CharSync::decode(sub.data);
                }
                _ => {}
            }
        }
        op if op == s2c::ENTITY_UPDATE2 => {
            if let Ok(pet) = decode::PetSync::decode(sub.data) {
                // Despawn variant: owner has no active pet. Nothing to
                // upsert — the pet's CHAR_NPC removal stream will clean
                // up the entity record on its own.
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
            // is_029 = true: Data/Data2 sit at offsets 8/12 (before ActIndex
            // pair). See `decode_battle_message` doc for the two layouts.
            if let Some(line) = decode_battle_message(sub.data, name_cache, true) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
            emit_battle_message_audio_event(sub.data, true, event_tx);
        }
        op if op == s2c::BATTLE_MESSAGE2 => {
            // 0x02D moves Data/Data2 to offsets 12/16, after the ActIndex pair.
            if let Some(line) = decode_battle_message(sub.data, name_cache, false) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
            emit_battle_message_audio_event(sub.data, false, event_tx);
        }
        op if op == s2c::SHOP_LIST => {
            // 0x03C body = 4 header + 10*N item rows. Reassembly across
            // multiple packets (>19 items) lands when we see one in the
            // wild; today the fold replaces the previous list outright.
            if let Some(shop) = decode_shop_list(sub.data) {
                let _ = event_tx.send(AgentEvent::ShopUpdated { shop });
            }
        }
        op if op == s2c::SHOP_OPEN => {
            // 0x03E carries no item data — it just signals "the list you
            // accumulated so far is final, render the window now". We
            // translate this to a flag flip on the existing ShopState
            // via a tiny helper event-fold rather than introducing a new
            // event variant for what is essentially observability.
            // Implementation: emit ShopUpdated with the current shop +
            // opened=true. Since we don't have the previous state here,
            // the fold downstream is what flips opened; we just signal.
            //
            // Simpler: rely on the `items.is_empty()` check in the HUD —
            // a non-empty list is enough to draw. The opened flag is
            // an extra hint; a follow-up can plumb it in if needed.
        }
        op if op == s2c::BATTLE2 => {
            // Bitpacked combat-action stream. One packet can carry
            // multiple targets and multiple results per target — fan
            // them out into individual chat lines so each hit/miss/
            // damage event lands separately in Chat 2.
            //
            // Pre-pass: emit `ActionStarted` so the viewer can load
            // the action's DAT and drive animation + particles + audio.
            // The header is the first 109 bits of the bitstream; we
            // peek just the actor + cmd_no + cmd_arg fields.
            if let Some((actor_id, action_id, action_kind)) = decode_battle2_header(sub.data) {
                let _ = event_tx.send(AgentEvent::ActionStarted {
                    actor_id,
                    action_id,
                    action_kind,
                });
            }
            for line in decode_battle2_action(sub.data, name_cache) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
        }
        op if op == s2c::MUSIC => {
            // 0x05F `GP_SERV_COMMAND_MUSIC` — 4-byte body:
            // `u16 Slot, u16 MusicNum`. Slot indexes LSB's
            // `MusicSlot` enum; the viewer-core BGM system decides
            // which slot is currently audible. See
            // `vendor/server/src/map/packets/s2c/0x05f_music.{h,cpp}`.
            if sub.data.len() >= 4 {
                let slot = u16::from_le_bytes([sub.data[0], sub.data[1]]) as u8;
                let track_id = u16::from_le_bytes([sub.data[2], sub.data[3]]);
                tracing::info!(slot, track_id, "0x05F MUSIC packet");
                let _ = event_tx.send(AgentEvent::MusicChanged { slot, track_id });
            }
        }
        op if op == s2c::MUSIC_VOLUME => {
            // 0x060 `GP_SERV_COMMAND_MUSICVOLUME` — same 4-byte
            // layout as 0x05F but the second field is a volume
            // value. We pass it through as u8 (LSB sends u16, but
            // the actual range is 0..=127 — viewer normalises).
            if sub.data.len() >= 4 {
                let slot = u16::from_le_bytes([sub.data[0], sub.data[1]]) as u8;
                let volume = u16::from_le_bytes([sub.data[2], sub.data[3]]) as u8;
                tracing::info!(slot, volume, "0x060 MUSIC_VOLUME packet");
                let _ = event_tx.send(AgentEvent::MusicVolumeChanged { slot, volume });
            }
        }
        op if op == s2c::WPOS || op == s2c::WPOS2 => {
            // Server-initiated forced position for the local player. LSB
            // emits this on cutscene end (0x05c), zone-line re-anchor
            // (0x05e), homepoint, GM warp. POSMODE selects what the
            // client should do; only NORMAL/EVENT/POP/RESET/MATERIALIZE
            // re-anchor the player. See ffxi_proto::decode::ForcedMove
            // for the body layout and the LSB vendor source cites.
            //
            // Knockback (BATTLE2 0x028 result.knockback) is intentionally
            // NOT routed here — it's an animation hint integrated
            // client-side, not a wire forced-move. The synthetic test in
            // reactor.rs exercises the same code path against this
            // event so the override semantics are still covered.
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
                    // Default override window: 1 second. The retail
                    // client's POP/MATERIALIZE animation lasts roughly
                    // that long; for instant teleports (NORMAL/EVENT)
                    // it's a no-op the lerp finishes in the first tick.
                    // A future enhancement can vary this per mode.
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
            // 0x057 — current zone weather. 8-byte fixed body:
            // `u32 StartTime, u16 WeatherNumber, u16 OffsetTime`. We only
            // surface `WeatherNumber` today; StartTime/OffsetTime would
            // let a future HUD render a "time until next change" hint
            // but no consumer needs them yet.
            if let Ok(w) = decode::WeatherPacket::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::WeatherUpdated {
                    weather_number: w.weather_number,
                });
            }
        }
        op if op == s2c::MISCDATA => {
            // 0x063 multiplexes by `type:u16`. Today we only consume
            // type=0x09 STATUS_ICONS — other types (Merits, JobPoints,
            // Homepoints, Unity) are nice-to-have HUD data but the
            // decoder for each lives behind a per-type switch we'll
            // grow as features need them.
            if let Some(icons) = decode_miscdata_status_icons(sub.data) {
                let _ = event_tx.send(AgentEvent::StatusIconsUpdated { icons });
            }
        }
        op if op == s2c::EVENT => {
            // 0x032 layout per server header:
            //   u32 UniqueNo, u16 ActIndex, u16 EventNum, u16 EventPara,
            //   u16 Mode, u16 EventNum2, u16 EventPara2 = 16 bytes.
            if let Some(dialog) = decode_event_0x032(sub.data) {
                emit_event_dialog(event_tx, &dialog, pending_event_end, name_cache);
            }
        }
        op if op == s2c::EVENTSTR => {
            // 0x033: 0x032 prefix + char String[4][16] + u32 Data[8].
            if let Some(dialog) = decode_event_0x033(sub.data) {
                emit_event_dialog(event_tx, &dialog, pending_event_end, name_cache);
            }
        }
        op if op == s2c::EVENTNUM => {
            // 0x034: u32 UniqueNo, i32 num[8], u16 ActIndex, u16 EventNum,
            // u16 EventPara, u16 Mode, u16 EventNum2, u16 EventPara2.
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
            // `Kind:u8 Attr:u8 Data:u16 sName[15] Mes[var]`. Mes is variable
            // — it runs to the sub-packet's reported length, which `framing`
            // already trimmed for us in `sub.data`. Trim trailing NULs from
            // both name and message; the server zero-pads.
            if let Some(line) = decode_chat_std(sub.data) {
                let _ = event_tx.send(AgentEvent::ChatLine { line });
            }
        }
        op if op == s2c::SYSTEMMES => {
            // 0x053 — formatted system message. Used by the /logout (id=7)
            // and /shutdown (id=35) tickers, plus a handful of trust /
            // treasure-pool messages. Look up the text in `msg_system`
            // and substitute `<seconds>`/`<number>` from para/para2;
            // unknown ids fall through to a visible `[system] msg #N`
            // line so we never silently drop a packet.
            if let Ok(m) = decode::SystemMessage::decode(sub.data) {
                let line = build_system_message_line(m);
                // CouldNotEnter (id=2; ids 0,1,3,4 share text per
                // vendor/server/src/map/enums/msg_std.h:30) is the
                // server's signal that a MAPRECT was rejected. Elevate
                // to info! so denials are visible without --trace.
                if m.message_id <= 4 {
                    tracing::info!(
                        msg_id = m.message_id,
                        text = %line.text,
                        "0x053 SYSTEMMES: server denied zone change",
                    );
                } else if m.message_id == 7 || m.message_id == 35 {
                    // EXECUTING_LOGOUT (id=7) / EXECUTING_SHUTDOWN (id=35) —
                    // the only positive ack the server sends for an accepted
                    // 0x0E7 ReqLogout. Elevate to info! so its presence or
                    // absence (validator rejected; GM/Mog-House immediate
                    // disconnect) is visible without RUST_LOG=trace.
                    tracing::info!(
                        msg_id = m.message_id,
                        seconds = m.para,
                        text = %line.text,
                        "0x053 SYSTEMMES: leavegame countdown tick",
                    );
                    // Drive the HUD countdown widget. We clamp `para` into
                    // u16 because the server's wire field is u32 but real
                    // values are always ≤30 — clamp keeps `as` cast lints
                    // honest and matches the LSB effect's domain.
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
                // System chat: rare (once per login + zone). Show only
                // non-zero capacities so the line stays readable on
                // characters with the default 30/30/0/0/... bag setup.
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
                            // ITEM_LIST doesn't carry price; ITEM_ATTR does.
                            // Default to 0; if the slot already has a price
                            // from a prior ITEM_ATTR it'll be overwritten,
                            // which is fine — bazaar prices are for trading
                            // only and the agent doesn't use them.
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
                // The 24-byte extdata payload (`a.extdata`) is discarded
                // here — it's item-type-specific (augments, charges, etc.)
                // and not needed for v1 banking decisions.
            }
        }
        op if op == s2c::EQUIP_CLEAR => {
            // 0x04F: server-side reset of the entire equipped-slot
            // table. Always sent before the per-slot 0x050 flood on
            // login (see `vendor/server/src/map/packets/c2s/0x00a_login.cpp`),
            // so the client never accumulates stale state from a
            // prior session.
            let _ = event_tx.send(AgentEvent::EquipCleared);
        }
        op if op == s2c::EQUIP_LIST => {
            // 0x050: one equipped slot. Body is 4 bytes
            // (container_index, equip_slot, container, padding) — see
            // `ffxi_proto::decode::EquipList`.
            if let Ok(e) = decode::EquipList::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::EquipUpdated {
                    slot: e.equip_slot,
                    container: e.container,
                    container_index: e.container_index,
                });
            }
        }
        op if op == s2c::MAGIC_DATA => {
            // 0x0AA: 128-byte bitmap of learned spells. Collapse into
            // a sorted `Vec<u16>` of ids the HUD can iterate.
            if let Ok(m) = decode::MagicData::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::SpellsKnownUpdated { ids: m.known_ids() });
            }
        }
        op if op == s2c::COMMAND_DATA => {
            // 0x0AC: four bitmaps (WeaponSkills/JobAbilities/PetAbilities/Traits).
            // Drop Traits — they're passive and surface via 0x063
            // STATUS_ICONS instead of as menu rows.
            if let Ok(c) = decode::CommandData::decode(sub.data) {
                let _ = event_tx.send(AgentEvent::CommandDataUpdated {
                    weapon_skills: decode::collect_set_bits(c.weapon_skills),
                    job_abilities: decode::collect_set_bits(c.job_abilities),
                    pet_abilities: decode::collect_set_bits(c.pet_abilities),
                });
            }
        }
        _ => {
            // Surface unknown opcodes at debug level; not an error.
            tracing::trace!(
                opcode = format!("0x{:03x}", sub.opcode),
                len = sub.data.len(),
                "unhandled sub-packet"
            );
        }
    }
}

/// Window during which a repeat miss for the same `(entity_id, miss_kind)`
/// pair is suppressed. 30s is long enough that a re-enrolled spawn or
/// rename emits a fresh miss event for the same entity, but short enough
/// that genuine stuck-state retries still surface inside a debugging
/// session.
const NAME_MISS_DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

/// Grace period for the `pending_event_end` watchdog under
/// `user_driven_events = true`. After this much wall time with the
/// queue non-empty (i.e. the operator hasn't issued `/endcutscene` or
/// `/release`), the keepalive auto-flushes the queue. 10s is long
/// enough to read most dialog HUD strings, short enough that an
/// unattended session doesn't sit in `BlockedState::InEvent` long.
const PENDING_EVENT_END_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// Cap on hex bytes captured per miss. CHAR_PC slot starts at 0x5A and
/// CHAR_NPC at 0x30; 96 bytes covers both with room for the trailing
/// 16-byte name slot.
const NAME_MISS_BODY_HEX_CAP: usize = 96;

/// Emit a `NameExtractionMiss` event (rate-limited per `(id, kind)`) when
/// `PosHead::try_extract_name` returned `None`. The event carries enough
/// raw context — the SendFlg byte and a hex dump of the leading body
/// bytes — that an attach-mode auditor can inspect the packet via the
/// `debug://name_misses` MCP resource without rebuilding the client.
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
    // Operator-side stderr log mirrors what the MCP resource will show,
    // so a local-only run (no MCP attached) still gets debug info via
    // `RUST_LOG=ffxi_client=debug`.
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
    mut name_miss_dedup: std::collections::HashMap<
        (u32, crate::state::NameMissKind),
        std::time::Instant,
    >,
    mut self_pos: Position,
    mut npc_name_resolver: NpcNameResolver,
) -> Result<MapOutcome> {
    let mut last_recv = std::time::Instant::now();
    // 100 ms tick gives the spec's ~10 Hz Move cadence under sustained
    // movement. POS subpacket inclusion is further gated by `should_emit_pos`
    // so heading-only / big-jump events can still bypass the rate-limit.
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
    tick.tick().await;
    let mut reconnect_addr: Option<std::net::SocketAddr> = None;
    let mut terminal_disconnect = false;
    // Watchdog for `0x05E MAPRECT`. LSB's `validate().blockedBy(InEvent)`
    // path drops the packet without sending `0x053 SYSTEMMES`, so a stuck
    // server-side event flag would otherwise be invisible. We surface it
    // as a chat-banner warning after 3s of no zone change.
    let mut pending_maprect: Option<(std::time::Instant, u32)> = None;
    // Watchdog for `pending_event_end` under `user_driven_events=true`
    // (native viewer). The dialog HUD wants events to stay alive long
    // enough to read; the operator dismisses with `/endcutscene` /
    // `/release`. But if neither fires within the grace period, the
    // server stays in `BlockedState::InEvent` and `/logout` + zone-change
    // get silently rejected (see `0x0e7_reqlogout.cpp::validate` and
    // `0x05e_maprect.cpp::validate`). The watchdog auto-drains the queue
    // so unattended sessions don't get permanently wedged. `None` while
    // the queue is empty; `Some(t)` from the first non-empty tick.
    let mut pending_event_end_since: Option<std::time::Instant> = None;
    // `/heal` mirror. Sources of truth:
    // 1. Optimistic write on outbound `AgentCommand::Heal`.
    // 2. Authoritative sync from CHAR_PC for self when UPDATE_HP gate set —
    //    `animation == ffxi_proto::decode::animation::HEALING (33)`.
    // 3. Implicit cancel: any keepalive tick that would advertise a new
    //    position prepends `0x0E8 Mode::Off` and clears this flag, so
    //    movement intent (WASD, /pathto, reactor goals) ends the rest
    //    without an explicit `/heal off`. Matches retail behavior.
    let mut is_healing = false;
    // Tracks the last position we keepalived so the heal-cancel
    // interceptor can detect "this tick advertises new coords." Seeded
    // with the spawn coords so the very first keepalive doesn't
    // false-trigger.
    let mut last_keepalive_pos: Vec3 = self_pos.pos;
    // 10 Hz Move emission state. `last_move_emission` tracks the last
    // outbound POS subpacket; `last_emitted_pos` / `last_emitted_heading`
    // back the big-jump and heading-changed bypass gates in
    // `should_emit_pos`. Initially `None` so the very first tick emits
    // unconditionally (server expects an authoritative position handshake
    // right after zone-in).
    let mut last_move_emission: Option<std::time::Instant> = None;
    let mut last_emitted_pos: Vec3 = self_pos.pos;
    let mut last_emitted_heading: u8 = self_pos.heading;
    // Active rubber-band target. Set by `reconcile_self_pos` when an
    // inbound CHAR_PC for self lands in the (2, 10] yalm correction band;
    // each tick lerps `self_pos.pos` toward it at 5 yalm/s until reached.
    let mut rubber_band_target: Option<Vec3> = None;
    let mut last_rubber_band_step: std::time::Instant = std::time::Instant::now();
    // Edge-detected self `MoghouseFlg`. Seeded to `false`; the first self
    // GROUP_ATTR / GROUP_LIST tick after entry inverts it and `note_mog_transition`
    // emits the explanatory chat line. Persists across the loop's many
    // iterations — across a normal zone change the value carries forward,
    // and the *next* self party tick (which will arrive shortly after the
    // new zone's LOGIN flood) will either re-affirm `true` (rezone into a
    // mog house) or flip it back to `false`.
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
                     Some(AgentCommand::StopMove) => { /* keepalive resends current pos */ }
                     Some(AgentCommand::SetFps { max }) => {
                         let _ = event_tx.send(AgentEvent::SetFps { max });
                     }
                     Some(AgentCommand::EndEvent) => {


                        // Flush all pending event-ends now (in addition to the next keepalive bundle).
                        if !pending_event_end.is_empty() {
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
                        }
                    }
                    Some(AgentCommand::EndEventChoice {
                        event_id,
                        act_index,
                        event_num,
                        choice,
                    }) => {
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
                        // Also drop this event from the auto-drain queue so
                        // the keepalive tick doesn't double-end it.
                        pending_event_end.retain(|(uid, _, en)| {
                            !(*uid == event_id && *en == event_num)
                        });
                        let _ = event_tx.send(AgentEvent::EventEnded);
                    }
                    Some(AgentCommand::Disconnect) => {
                        let _ = event_tx.send(AgentEvent::Disconnected { reason: "agent requested disconnect".into() });
                        break;
                    }
                    Some(AgentCommand::ReqLogout { kind }) => {
                        // 0x0E7 ReqLogout. We do *not* break out of the
                        // session loop — the server runs `EFFECT_LEAVEGAME`
                        // (≈30s for normal players, immediate for GMs /
                        // Mog House per `scripts/effects/leavegame.lua`),
                        // and the closing s2c 0x00B LOGOUT
                        // (state != ZONECHANGE) is what actually drops
                        // us. Tearing down here would skip the server's
                        // `blockedBy: { InEvent, AbnormalStatus, Crafting,
                        // PreventAction }` validator — exactly the
                        // misuse `AgentCommand::Disconnect` is for.
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
                        // Snapshot is the "resync hammer" — emit a burst
                        // covering every transitional event that an
                        // attach-mode consumer might have missed before
                        // subscribing. Periodic events (Diagnostics,
                        // entity upserts) come through naturally; only
                        // the one-shot transitions need this kick.
                        //
                        // Without this burst, a late-attaching MCP sees
                        // `state.stage == Stage::Idle` forever, and
                        // `scene://current` reports "Session not started."
                        // even though packets are flowing — because
                        // `state.stage` is only updated by `StageChanged`,
                        // not by `Diagnostics`.
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
                    Some(AgentCommand::ReturnToHomePoint) => {
                        // The 0x01A action's `UniqueNo`/`ActIndex` are
                        // ignored by Phoenix for this action_id, but we
                        // still need a non-zero `ActIndex` for the
                        // packet to look well-formed. Fall back to 0 if
                        // we haven't seen our own CHAR_PC yet — the
                        // server-side handler doesn't read it.
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
                            // Optimistic local mirror. The authoritative
                            // source is the next CHAR_PC for self with
                            // `animation` byte under the UPDATE_HP gate —
                            // server validation may reject our request
                            // (engaged / in-event / etc.) and CHAR_PC
                            // will reconcile within a tick or two. For
                            // Toggle, we flip; for On/Off, we set.
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
                        // 0x050 GP_CLI_COMMAND_EQUIP_SET. Wire payload is
                        // 3 bytes (PropertyItemIndex, EquipKind, Category)
                        // — see `vendor/server/src/map/packets/c2s/0x050_equip_set.h`.
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
                        // 0x037 GP_CLI_COMMAND_ITEM_USE. UniqueNo carries
                        // the recipient (target) UniqueNo — same target as
                        // the AI-side `PChar->GetEntity(this->ActIndex)`
                        // lookup in Phoenix/0x037_item_use.cpp. ActIndex is
                        // the recipient's per-zone ActIndex.
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
                        // `RectID="zmrq"` MAPRECT. Server gates this on the
                        // player actually being inside a Mog House
                        // (`PChar->inMogHouse()` at `0x05e_maprect.cpp:134`);
                        // calling it outside is harmless (the server logs
                        // "Moghouse zoneline abuse" and ignores it) but
                        // still costs a watchdog timeout. We *don't* gate
                        // client-side because `MoghouseFlg` may not have
                        // arrived yet during a reconnect — the operator
                        // who types `/mhexit` knows what they want.
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
                            // Reuse the same watchdog — a sentinel line_id
                            // makes the timeout chat-line still useful (the
                            // chat line literally calls out "Zone change for
                            // line 0x7A6D7271 silently dropped" — the operator
                            // sees the `zmrq` bytes echoed back, confirming
                            // which packet was lost).
                            const ZMRQ_LE: u32 =
                                u32::from_le_bytes([b'z', b'm', b'r', b'q']);
                            pending_maprect =
                                Some((std::time::Instant::now(), ZMRQ_LE));
                        }
                        bundle_seq = bundle_seq.wrapping_add(1);
                    }
                }
            }
            _ = tick.tick() => {
                // MAPRECT watchdog: if a zone change was requested >3s ago
                // and we're still in the same zone, surface a chat-visible
                // warning so the operator knows the server silently dropped
                // the packet. See `vendor/server/src/map/packets/c2s/validation.cpp:46`
                // — `BlockedState::InEvent` rejects MAPRECT without emitting
                // `0x053 SYSTEMMES`, leaving the failure otherwise invisible.
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
                // Sync the watchdog timer from the queue's current
                // shape. Empty queue → no pending timer; non-empty
                // queue with no timer → start the clock now. 1Hz sync
                // adds at most one second of error to the grace
                // window, which is fine.
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
                // Build a bundle: any auto-event-ends drained, then a POS keepalive.
                // `user_driven_events` suppresses the auto-drain so the dialog HUD
                // gets a chance to display the event; the operator (or HUD-side
                // input handler) sends `EndEvent` explicitly to advance — unless
                // the watchdog grace expires, in which case we drain anyway so
                // the server's `BlockedState::InEvent` doesn't permanently wedge
                // /logout + zone-change.
                let mut payload = Vec::new();
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
                // Rubber-band advance: if an inbound CHAR_PC landed in the
                // (2, 10] yalm correction band, walk `self_pos.pos` toward
                // the stored target at 5 yalm/s. Done *before* the heal-
                // cancel / POS-gating below so the emitted position
                // already reflects the lerped value.
                if let Some(target) = rubber_band_target {
                    let dt = last_rubber_band_step.elapsed().as_secs_f32();
                    last_rubber_band_step = std::time::Instant::now();
                    let max_step = 5.0 * dt; // 5 yalm/s
                    let (next, reached) = lerp_toward(self_pos.pos, target, max_step);
                    self_pos.pos = next;
                    if reached {
                        rubber_band_target = None;
                    }
                } else {
                    last_rubber_band_step = std::time::Instant::now();
                }

                // Heal-cancel interceptor: if we're healing and this tick
                // would advertise a new position, prepend 0x0E8 Mode::Off
                // so the server clears `EFFECT_HEALING` *before* it sees
                // the position change. Without this, server-side cancel
                // would be racy/animation-dependent. Comparing on `.pos`
                // (not heading) — turning in place while healing is
                // visually allowed in retail; only translation cancels.
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
                // 10 Hz Move cadence gate. POS is included when EITHER:
                //   - `>= 100ms` since the last emission (sustained 10 Hz),
                //   - position delta from last emission > 0.5 yalm (jump),
                //   - heading byte changed (immediate flush).
                // First tick (`last_move_emission == None`) always emits so
                // the server's zone-in handshake has an authoritative pos.
                let dx = self_pos.pos.x - last_emitted_pos.x;
                let dy = self_pos.pos.y - last_emitted_pos.y;
                let dz = self_pos.pos.z - last_emitted_pos.z;
                let pos_delta = (dx * dx + dy * dy + dz * dz).sqrt();
                let heading_changed = self_pos.heading != last_emitted_heading;
                let include_pos = match last_move_emission {
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
                // Skip the network send when there's nothing to put on the
                // wire — at 100ms tick we'd otherwise pump empty UDP
                // bundles 10× a second. The 100ms POS gate ensures a
                // bundle goes out at least every 100ms under normal
                // conditions; recv-side traffic keeps the connection live.
                if !payload.is_empty() {
                    if let Err(e) = map.send_encrypted(&payload, bundle_seq, server_last_seq).await {
                        tracing::warn!(error = %e, "keepalive send failed");
                        let _ = event_tx.send(AgentEvent::Error { message: format!("keepalive send: {e}") });
                        break;
                    }
                    bundle_seq = bundle_seq.wrapping_add(1);
                }
            }
            res = tokio::time::timeout(std::time::Duration::from_millis(50), map.recv_decrypted()) => {
                if let Ok(Ok(buf)) = res {
                    last_recv = std::time::Instant::now();
                    let header = framing::Header::read(&buf[..framing::FFXI_HEADER_SIZE]);
                    server_last_seq = header.id_and_size;
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
                            // Snapshot the local self-position *before*
                            // dispatching to `handle_sub_packet` — that
                            // handler unconditionally overwrites
                            // `self_pos` with the server's value on a
                            // CHAR_PC for self. We want to apply
                            // rubber-band reconciliation against the
                            // pre-overwrite local pos, so capture it
                            // here.
                            let prev_self_pos = self_pos.pos;
                            handle_sub_packet(
                                &sub,
                                &event_tx,
                                &mut pending_event_end,
                                self_char_id,
                                &character_name,
                                &mut self_act_index,
                                &mut name_cache,
                                &mut name_miss_dedup,
                                &mut current_zone_id,
                                &mut self_pos,
                                &mut npc_name_resolver,
                                &mut self_in_mog_house,
                            );
                            // Self-reconciliation (rubber-band). The
                            // handler just clobbered `self_pos` with the
                            // server's PosHead for self; decide whether
                            // to keep that, ignore it (trust local), or
                            // gradually correct toward it.
                            if sub.opcode == ffxi_proto::map::s2c::CHAR_PC {
                                if let Ok(head) = decode::PosHead::decode(sub.data) {
                                    if head.unique_no == self_char_id {
                                        let server_pos = self_pos.pos;
                                        match reconcile_self_pos(prev_self_pos, server_pos) {
                                            SelfPosReconcile::KeepLocal => {
                                                // Sub-yalm jitter — trust the
                                                // local integrator and ignore
                                                // the server's pos. Cancels
                                                // any in-flight rubber-band
                                                // since we're already inside
                                                // the tolerance band.
                                                self_pos.pos = prev_self_pos;
                                                rubber_band_target = None;
                                            }
                                            SelfPosReconcile::Rubberband { target } => {
                                                // Mid-band correction — keep
                                                // the local pos visible now,
                                                // close the gap at 5 yalm/s
                                                // over subsequent ticks.
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
                                                // Zone teleport / GM warp —
                                                // server's pos already wrote;
                                                // just clear any pending
                                                // rubber-band that became
                                                // irrelevant.
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
                            // Heal-mirror sync from CHAR_PC for self. Same
                            // UPDATE_HP gate (`body[6] & 0x04`) that
                            // authorizes `hpp` also authorizes the
                            // animation byte at body[27] — without the
                            // gate, position-only ticks would clobber
                            // our local mirror with stale zero. Done
                            // here (not inside `handle_sub_packet`) so
                            // the handler's signature doesn't have to
                            // know about session-loop-local state.
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

        if last_recv.elapsed() > std::time::Duration::from_secs(60) {
            let _ = event_tx.send(AgentEvent::Disconnected {
                reason: "no server packets for 60s".into(),
            });
            break;
        }
    }

    // We do NOT drop `map` here — the outer `run` reuses the same
    // socket across zone-change reconnects via `MapClient::retarget`.
    // See `MapClient::retarget` for the LSB single-process rationale.

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

/// Render a `0x053 SYSTEMMES` packet as a chat line. The message text
/// comes from `xi.msg.system` and substitution fills `<seconds>` /
/// `<number>` / `<param>` from `para`/`para2`. Unknown ids surface as
/// `[system] msg #N para=A,B` so a missing scrape entry is visible
/// rather than silent.
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

/// Replace numeric placeholders in `xi.msg.system` text with `para` /
/// `para2`. The /logout countdown uses `<seconds>`; treasure-pool gil
/// and trust messages use `<number>` / `<param>` / `<value>`. We map
/// every numeric placeholder we've seen so far to `para`, with
/// `<number2>` reaching `para2` to mirror the battle-message pattern.
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

/// `GP_SERV_COMMAND_BATTLE_MESSAGE` (0x029) and `BATTLE_MESSAGE2` (0x02D)
/// share the same set of fields but in two different orderings:
///
/// - 0x029: `u32 UniqueNoCas, u32 UniqueNoTar, u32 Data, u32 Data2,
///   u16 ActIndexCas, u16 ActIndexTar, u16 MessageNum, u8 Type, u8 pad`
/// - 0x02D: `u32 UniqueNoCas, u32 UniqueNoTar, u16 ActIndexCas, u16 ActIndexTar,
///   u32 Data, u32 Data2, u16 MessageNum, u8 Type, u8 pad`
///
/// Both are 24 bytes. `is_029` selects the layout. The text is looked up
/// in `ffxi_proto::msg_basic`; `<user>`/`<target>`/`<amount>` placeholders
/// are substituted against `name_cache` (session-local id→name table)
/// and the `Data`/`Data2` fields. Returns `None` if the body is too short
/// or the message id has no entry in `msg_basic` (rare ids live in
/// `msg_combat` / `msg_status` tables we don't ship yet).
/// Sibling to [`decode_battle_message`] — peeks at the same packet
/// body and emits audio-trigger `AgentEvent`s for the message ids
/// that the SFX bridge cares about. Today: LevelUp (9), SkillLevelUp
/// (53). The chat-line decoder handles rendering; this fires the
/// stinger.
///
/// Source-of-truth for ids: `vendor/server/src/map/enums/msg_basic.h`
/// (LevelUp=9, SkillLevelUp=53). Source-of-truth for `data1`/`data2`
/// offsets: `decode_battle_message`'s `is_029` switch.
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
            // charutils.cpp:4161 sends (skillID, (skill+amount)/10) —
            // data1 = skill_id, data2 = current level (server already
            // divided by 10).
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

    // /check responses live in the client's `msg_basic.dat` (which we
    // don't ship), but their semantic content is fully numeric on the
    // wire — `synth_check_line` reconstructs the English locally from
    // `data1`/`data2`/`message_num` rather than waiting on a DMSG
    // decoder. Covers /check on mobs (170-178) and /checkparam
    // (712-733); pass-throughs to the placeholder path for everything
    // else.
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
    let text =
        substitute_battle_placeholders(raw, &cas_name, &tar_name, data1, data2, message_num, None);
    Some(ChatLine {
        channel: ChatChannel::Battle,
        // Use the actor's name as the sender so the chat row reads
        // "Cas: hits Tar for X damage." — same shape as social channels.
        // For messages where the grammatical subject is the wire `tar`
        // (e.g. PlayerDefeatedBy), prefer that name so the chat row
        // header still names the right entity.
        sender: if subject_is_tar(message_num) {
            tar_name
        } else {
            cas_name
        },
        text,
        server_ts: 0,
    })
}

/// Bit reader mirroring LSB's `unpackBitsBE`
/// (`vendor/server/src/common/utils.cpp:336`). The misleading "BE" name
/// refers to the field-packing convention, NOT byte endianness:
/// - Multi-byte fields are stored in native **little-endian** byte order
///   (LSB reinterprets the byte stream as `uint16*`/`uint32*`/`uint64*`).
/// - Within a byte, the low bits hold the first-packed field; subsequent
///   fields ride above them.
///
/// Algorithm: read a u16/u32/u64 from the byte covering the cursor,
/// shift right by the within-byte offset, mask off the field width.
/// Cursor advances bit-by-bit so we stay aligned across the conditional
/// proc/react sub-blocks in 0x028.
///
/// `read(n)` returns up to 32 bits at a time (matching what the 0x028
/// format ever packs in a single field; max width = 32). Underflow on
/// the underlying buffer returns `None` so the caller drops the rest
/// of the action rather than fabricate a partial chat line.
struct BattleBitReader<'a> {
    data: &'a [u8],
    pos: usize, // bit position
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
            // total_bits up to 39 (32 width + 7 within-byte) needs a u64 read.
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

/// Decode a `GP_SERV_COMMAND_BATTLE2` (0x028) body into a stream of
/// chat lines — one per `result` carried by the action. The wire
/// format is bitpacked BE. Variable structure: up to 15 targets × 8
/// results each, plus per-result optional `proc` and `react`
/// sub-blocks that have to be skipped (not just ignored) to keep the
/// bit cursor aligned for the next target.
///
/// Bit-offset convention: LSB's `pack()`/`unpack()` operate on the
/// full sub-packet buffer (4-byte SE header + 1-byte workSize +
/// bitstream) and start at bit offset `8 * 5 = 40`. Our `SubPacket::data`
/// (`ffxi-proto/src/framing.rs:157`) already strips the 4-byte
/// sub-packet header, so the bitstream starts at bit `8` of `data`
/// (skip only the `workSize` byte at `data[0]`). Reading from bit 40
/// here would skip 4 extra bytes of payload and decode garbage.
///
/// Returns an empty Vec if the body is too short or the bit cursor
/// underflows mid-decode. Returns a partial list if mid-stream
/// truncation hits after some results were already emitted — the
/// partial ones are valid.
/// Peek the action-start header of a `GP_SERV_COMMAND_BATTLE2` (0x028)
/// sub-packet body: `(actor_id, action_id /* cmd_arg */, action_kind /* cmd_no */)`.
///
/// Layout (mirrors [`decode_battle2_action`] start; see that function for
/// the full bitstream documentation). We deliberately stop after the
/// header so this can run cheaply per packet without spinning up the
/// full target/result iteration. Returns `None` on truncation —
/// callers should fall through to chat-line decoding regardless.
pub fn decode_battle2_header(data: &[u8]) -> Option<(u32, u32, u8)> {
    let mut br = BattleBitReader::new(data, 8); // skip workSize byte
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
) -> Vec<ChatLine> {
    let mut out: Vec<ChatLine> = Vec::new();
    // Skip the 1-byte workSize prefix; the bitstream proper starts at bit 8.
    // See doc comment above for why this is 8 and not 40.
    let mut br = BattleBitReader::new(data, 8);

    let actor_id = match br.read(32) {
        Some(v) => v as u32,
        None => return out,
    };
    let trg_sum = br.read(6).unwrap_or(0) as usize;
    let _res_sum = br.read(4); // unused by client (always 0)
    let _cmd_no = br.read(4); // command type (4 = spell, 6 = ability, etc.)
    let cmd_arg = match br.read(32) {
        Some(v) => v as u32,
        None => return out,
    };
    let _info = br.read(32); // recast etc.

    let cas_name = name_for_id(actor_id, name_cache);

    for _t in 0..trg_sum.min(15) {
        let Some(target_id) = br.read(32) else {
            return out;
        };
        let result_sum = br.read(4).unwrap_or(0) as usize;
        let tar_name = name_for_id(target_id as u32, name_cache);

        for _r in 0..result_sum.min(8) {
            // Core result fields. Layout from `unpack`:
            //   miss/resolution: 3 bits
            //   kind: 2
            //   sub_kind (animation): 12
            //   info: 5
            //   scale: 5  (hitDistortion 2 + knockback 3)
            //   value (damage/heal amount): 17
            //   message: 10
            //   bit (modifier): 31
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

            // Headline result. message_num=0 means "no message" — common
            // for purely-cosmetic animation results (e.g., the cast-
            // animation half of a spell that also carries a damage
            // result in a sibling slot). Drop those.
            if message_num != 0 {
                if let Some(line) =
                    build_battle2_line(message_num, &cas_name, &tar_name, value, cmd_arg)
                {
                    out.push(line);
                }
            }
            // Additional-effect line (proc): "Additional effect: ..."
            // Usually packed with its own messageID like 163.
            if has_proc && proc_message != 0 {
                if let Some(line) =
                    build_battle2_line(proc_message, &cas_name, &tar_name, proc_value, cmd_arg)
                {
                    out.push(line);
                }
            }
            // Reaction line (spikes/parry/etc.).
            if has_react && react_message != 0 {
                if let Some(line) =
                    build_battle2_line(react_message, &cas_name, &tar_name, react_value, cmd_arg)
                {
                    out.push(line);
                }
            }
        }
    }

    out
}

/// Build a single `ChatLine` from a 0x028 result triple. Returns
/// `None` if the message id isn't in `msg_basic` (rare ids live in
/// tables we don't ship yet — same fallback as `decode_battle_message`).
fn build_battle2_line(
    message_num: u16,
    cas_name: &str,
    tar_name: &str,
    amount: u32,
    action_id: u32,
) -> Option<ChatLine> {
    let raw = template_for_id(message_num)?;
    let text = substitute_battle_placeholders(
        raw,
        cas_name,
        tar_name,
        amount,
        0,
        message_num,
        Some(action_id),
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

/// Resolve a battle-message id to its template text. Consults a small
/// override table first for ids where LSB's `msg_basic.h` comment is
/// known to be lossy versus the retail localized template, then falls
/// back to the generated `msg_basic` table.
///
/// Each override entry must be backed by a citation: a call site where
/// LSB sends this id with semantics that the generic enum comment
/// doesn't capture. Without that citation, the override is a guess and
/// will rot the day LSB widens the id's usage.
fn template_for_id(message_num: u16) -> Option<&'static str> {
    for &(id, template) in TEMPLATE_OVERRIDES {
        if id == message_num {
            return Some(template);
        }
    }
    ffxi_proto::msg_basic::lookup(message_num)
}

/// Synthesize a chat line for /check (170-178), /check-on-PC
/// (CheckImpossibleToGauge=179 falls through to the normal table) and
/// /checkparam (712-733). These ids carry their full meaning in the
/// numeric `data1`/`data2`/`message_num` fields, so we render them
/// directly without needing the FFXI client's localized
/// `msg_basic.dat`.
///
/// Returns `None` for ids outside these ranges; callers should fall
/// through to the placeholder-substitution path.
///
/// **Wire layout**
/// - `/check` mob (LSB `0x0dd_equip_inspect.cpp:71-124`):
///   - `cas`/`tar` = player/mob, `data1` = mob level (`mobLvl`),
///     `data2` = `64 + EMobDifficulty`.
///   - `message_num` ∈ 170..=178, offset from 174 decomposes into a
///     defense and an evasion modifier per LSB's calc:
///     `defOffset = -1 (high), 0, +1 (low)` and
///     `evaOffset = -3 (high), 0, +3 (low)`. Sum unambiguously maps
///     back to one (def, eva) pair because |def| < 3.
/// - `/checkparam` (same file, `:155-181`):
///   - `cas`/`tar` = player/player (or player/pet).
///   - `data1` = ACC (or RACC, or EVA), `data2` = ATT (or RATT, or DEF)
///     depending on which sub-id was pushed.
///
/// **EMobDifficulty** (`vendor/server/src/map/utils/charutils.h:45-56`):
///   0 TooWeak, 1 IncrediblyEasyPrey, 2 EasyPrey, 3 DecentChallenge,
///   4 EvenMatch, 5 Tough, 6 VeryTough, 7 IncrediblyTough.
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

/// Render one of the 9 mob /check message ids (170..=178) into a
/// human-readable line. The id encodes a (def, eva) modifier pair
/// relative to 174 ("even/even"); `data1` carries the mob level;
/// `data2 - 64` is the `EMobDifficulty` enum value.
fn render_check_mob(message_num: u16, data1: u32, data2: u32, tar_name: &str) -> String {
    let total: i32 = message_num as i32 - 174;
    // Evasion is +/-3 (saturates the outer range); defense is +/-1.
    // |def| < 3 so `eva = round_to_3(total)` and `def = total - eva`.
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
        // Out-of-band difficulty; degrade gracefully rather than drop.
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

/// Cited overrides for ids whose `msg_basic.h` comment is lossy
/// relative to the retail template the client would render from its
/// own `msg_basic.dat`. Until we ship a DMSG parser, this is the
/// narrow workaround.
const TEMPLATE_OVERRIDES: &[(u16, &str)] = &[
    // MsgBasic::Obtains (565). LSB comment: "<target> obtains <amount>."
    // The only LSB call sites are gil distribution
    // (vendor/server/src/map/utils/charutils.cpp:4756, 4763) — both
    // pushPacket<...>(PChar, PChar, gilAmount, 0, MsgBasic::Obtains).
    // Retail's localized template carries the unit "gil", which the
    // header comment drops.
    (565, "<target> obtains <amount> gil."),
];

/// True when the wire-protocol's `Tar` slot holds the *grammatical subject*
/// of the message (the entity the message is "about") rather than the
/// `Cas` slot. Phoenix's `GP_SERV_COMMAND_BATTLE_MESSAGE` constructor
/// always packs `(PSender, PTarget) = (Cas, Tar)`, but for some message
/// ids the comment template's lead placeholder (`<player>`) refers to
/// `PTarget`, not `PSender` — e.g. `PlayerDefeatedBy = 97`,
/// `<player> was defeated by the <target>.`, where `PSender` is the
/// killer and `PTarget` is the victim.
///
/// New entries here should be backed by a Phoenix call site (grep for
/// `MsgBasic::<name>` in `vendor/Phoenix/src/`) — the call's argument
/// order tells you which slot is which.
fn subject_is_tar(message_num: u16) -> bool {
    matches!(
        message_num,
        97 // PlayerDefeatedBy: <player>=victim (Tar), <target>=killer (Cas)
    )
}

/// Resolve a `UniqueNo` to a display name via the session-local cache;
/// fall back to the hex id when the entity hasn't been seen yet (common
/// right after a zone-in before CHAR_PC/CHAR_NPC has flooded). `id == 0`
/// is the "no actor" sentinel some battle messages carry.
fn name_for_id(id: u32, name_cache: &std::collections::HashMap<u32, String>) -> String {
    if id == 0 {
        return "<no one>".to_string();
    }
    name_cache
        .get(&id)
        .cloned()
        .unwrap_or_else(|| format!("#{:08X}", id))
}

/// Substitute the FFXI placeholder tokens used in `msg_basic.h` comments.
/// We handle the high-frequency tokens directly; rare ones (`<spell>`,
/// `<item>`, `<job>`, …) require lookup tables we don't ship yet and are
/// left as-is so the operator at least sees the literal token rather
/// than a hidden gap.
///
/// `<player>` is special: in most templates it's the actor (slot Cas),
/// but for messages like `PlayerDefeatedBy` (97) it's the recipient
/// (slot Tar). `subject_is_tar(message_num)` flips the binding for
/// those exceptions.
fn substitute_battle_placeholders(
    raw: &str,
    cas_name: &str,
    tar_name: &str,
    data1: u32,
    data2: u32,
    message_num: u16,
    action_id: Option<u32>,
) -> String {
    let mut s = raw.to_string();
    // `<entity>` is the grammatical subject — same wire slot as `<user>`
    // (e.g., ReadiesWeaponskill 43: "<entity> readies <skill>." packs
    // the actor into Cas). Grouped with the actor placeholders so any
    // template using these reads as the actor's action.
    for tag in ["<user>", "<attacker>", "<caster>", "<entity>"] {
        s = s.replace(tag, cas_name);
    }
    let (player_name, target_name) = if subject_is_tar(message_num) {
        (tar_name, cas_name)
    } else {
        (cas_name, tar_name)
    };
    s = s.replace("<player>", player_name);
    s = s.replace("<target>", target_name);
    // `<mob>` appears in a handful of templates (e.g.
    // `CheckImpossibleToGauge = 249`, "<mob> strength is impossible to
    // gauge!") where the mob entity is in the wire's `Tar` slot —
    // LSB's call sites for these ids pass `(PChar, PMobTarget, ...)`.
    s = s.replace("<mob>", target_name);
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
    // Vendor-scraped name tables resolve `<spell>`, `<ability>`,
    // `<item>`, `<job>` against `action_id` (the canonical source on
    // 0x028 — cmd_arg at the action level) with a `data1` fallback
    // (the slot Phoenix uses on 0x029 BATTLE_MESSAGE). `<status>`
    // takes `data1` directly: GainsEffect-family templates pack the
    // status id into `param` on 0x029. For 0x028 action results the
    // status id lives in the result's `modifier`/`info` bits, which
    // `build_battle2_line` does not yet thread through — once those
    // surface, prefer them over data1.
    //
    // Unknown ids fall back to "spell #N" etc. so the operator at
    // least sees *which* spell/item the message refers to, rather
    // than a literal `<spell>` token that hides the gap.
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
    // Bare `X` / `#` are msg_basic.h's shorthand for inline numbers
    // (e.g., "skill rises X points.", "gains # experience points."). Use
    // token-boundary matching so words like "BoX" / hashtags don't get
    // mangled. `X` always reads `data2`. `#` reads `data1` for everything
    // except `ExpChain` (id 253), whose template carries two `#` markers
    // — `chain #!` is `data2` (chain number) and `gains # experience` is
    // `data1` (exp earned).
    if message_num == 253 {
        s = replace_marker_nth(&s, '#', 0, &data2.to_string());
        s = replace_marker_nth(&s, '#', 0, &data1.to_string());
    } else {
        s = replace_marker_all(&s, '#', &data1.to_string());
    }
    // SkillGain (38) and SkillDrop (310) carry the raw skill amount —
    // retail displays it as `raw/10` with one decimal ("rises 0.1
    // points" for amount 1). SkillLevelUp (53), LevelSync (540), and
    // ROETimed (705) all carry pre-divided integers, so default to
    // integer formatting for everything else.
    let x_value = if matches!(message_num, 38 | 310) {
        format_decimal_tenths(data2)
    } else {
        data2.to_string()
    };
    s = replace_marker_all(&s, 'X', &x_value);
    s
}

/// Render an integer count of tenths as `whole.tenth` (e.g., 1 → "0.1",
/// 23 → "2.3"). Matches retail FFXI's skill-display formatting where
/// the server emits raw amounts and the client divides by 10.
fn format_decimal_tenths(tenths: u32) -> String {
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// Replace every token-boundary occurrence of `marker` with `value`.
/// A "token boundary" means the marker is not adjacent to an alphanumeric
/// or `_` on either side — so `X` in "BoX" or `#` in a hex literal is
/// left alone, but `X` in "rises X points" or `#` in "gains #" is swapped.
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

/// Replace only the `n`-th (0-indexed) token-boundary occurrence of `marker`.
/// Returns the original string unchanged if there are fewer than `n+1`
/// matches. Used for `ExpChain` where the template carries two `#` markers
/// that map to different data slots.
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

/// `GP_SERV_COMMAND_EVENT` (0x032) decoder. Body layout per
/// `vendor/server/src/map/packets/s2c/0x032_event.h`:
/// `u32 UniqueNo, u16 ActIndex, u16 EventNum, u16 EventPara, u16 Mode,
///  u16 EventNum2, u16 EventPara2` = 16 bytes.
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
    })
}

/// `GP_SERV_COMMAND_EVENTSTR` (0x033) decoder. Body layout per
/// `vendor/server/src/map/packets/s2c/0x033_eventstr.h`:
/// 0x032 prefix (without EventNum2/EventPara2) + `char String[4][16]` +
/// `u32 Data[8]`. The strings are NUL-trimmed; runs of empty strings are
/// dropped from the tail to keep the surfaced list compact.
fn decode_event_0x033(data: &[u8]) -> Option<crate::state::DialogState> {
    // 12 (UniqueNo+ActIndex+EventNum+EventPara+Mode) + 64 (4×16 strings)
    // + 32 (8×u32) = 108 bytes minimum.
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

    // Data[8] runs at offset 12 + 64 = 76.
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
        // 0x033 doesn't carry EventNum2/EventPara2 — leave at default 0.
        event_num2: 0,
        event_para2: 0,
        strings,
        nums,
    })
}

/// `GP_SERV_COMMAND_EVENTNUM` (0x034) decoder. Body layout per
/// `vendor/server/src/map/packets/s2c/0x034_eventnum.h`:
/// `u32 UniqueNo, i32 num[8], u16 ActIndex, u16 EventNum, u16 EventPara,
///  u16 Mode, u16 EventNum2, u16 EventPara2` = 48 bytes.
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
    })
}

/// `GP_SERV_COMMAND_SHOP_LIST` (0x03C) decoder. Body layout per
/// `vendor/server/src/map/packets/s2c/0x03c_shop_list.h`:
///   u16 ShopItemOffsetIndex, u8 Flags, u8 pad, GP_SHOP[N]
///   GP_SHOP = u32 ItemPrice, u16 ItemNo, u8 ShopIndex, u8 pad,
///             u16 Skill, u16 GuildInfo  (12 bytes)
///
/// The number of items is implied by the body length: `(len - 4) / 12`.
/// Items beyond what fits in the body are dropped silently — the server
/// chunks lists >19 items across multiple packets, and the simple fold
/// replaces the prior list rather than appending. Reassembly lands when
/// we see a >19-item shop in practice.
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
        // Some servers pad the unused tail of the array with zeroed rows;
        // skip those (item_no == 0 is the sentinel) so the HUD doesn't
        // show empty rows. A real shop never sells item id 0.
        if item_no == 0 {
            continue;
        }
        items.push(ShopItem {
            price: u32::from_le_bytes(row[0..4].try_into().unwrap()),
            item_no,
            shop_index: row[6],
            // row[7] = padding byte
            skill: u16::from_le_bytes(row[8..10].try_into().unwrap()),
            guild_info: u16::from_le_bytes(row[10..12].try_into().unwrap()),
        });
    }
    Some(ShopState {
        offset_index,
        items,
        // 0x03E `SHOP_OPEN` flips this to true in a follow-up; phase 1
        // leaves it false — the HUD draws on `!items.is_empty()` anyway.
        opened: false,
    })
}

/// `GP_SERV_COMMAND_MISCDATA` (0x063) → STATUS_ICONS variant.
///
/// Body layout per `0x063_miscdata_status_icons.h`:
///
/// ```text
///   u16 type           // == 0x09 for StatusIcons; we ignore other types here
///   u16 unknown06      // server uses sizeof(PacketData)
///   u16 icons[32]      // 64 bytes
///   u32 timestamps[32] // 128 bytes  (ignored by the basic decoder)
/// ```
///
/// Total body = 196 bytes. `0x00FF` is the "no icon" placeholder; we
/// drop those from the returned vec. Returns `None` if the body is
/// truncated or the type field isn't `0x09`.
fn decode_miscdata_status_icons(data: &[u8]) -> Option<Vec<u16>> {
    const TYPE_OFFSET: usize = 0;
    const ICONS_OFFSET: usize = 4;
    const ICONS_COUNT: usize = 32;
    const ICONS_BYTES: usize = ICONS_COUNT * 2;
    const PLACEHOLDER: u16 = 0x00FF;

    if data.len() < ICONS_OFFSET + ICONS_BYTES {
        return None;
    }
    let kind = u16::from_le_bytes(data[TYPE_OFFSET..TYPE_OFFSET + 2].try_into().unwrap());
    if kind != 0x0009 {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..ICONS_COUNT {
        let off = ICONS_OFFSET + i * 2;
        let icon = u16::from_le_bytes(data[off..off + 2].try_into().unwrap());
        if icon != PLACEHOLDER && icon != 0 {
            out.push(icon);
        }
    }
    Some(out)
}

/// `GP_CLI_COMMAND_SHOP_BUY` (0x083) builder. 4-byte sub-packet header +
/// 12-byte body = 16 bytes (size_words = 4).
///
/// Body per `vendor/server/src/map/packets/c2s/0x083_shop_buy.h`:
///   u32 ItemNum (qty), u16 ShopNo, u16 ShopItemIndex,
///   u8 PropertyItemIndex, u8 pad[3]
///
/// `PropertyItemIndex` selects which of the player's containers to
/// deposit into; `0` (= LOC_INVENTORY) is the universal default and what
/// classic FFXI clients send for NPC purchases. We hard-code it for v1.
pub fn build_subpacket_shop_buy(sync: u16, qty: u32, shop_no: u16, shop_index: u8) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x083, 4, sync));
    buf[4..8].copy_from_slice(&qty.to_le_bytes());
    buf[8..10].copy_from_slice(&shop_no.to_le_bytes());
    buf[10..12].copy_from_slice(&(shop_index as u16).to_le_bytes());
    buf[12] = 0; // PropertyItemIndex = LOC_INVENTORY
    buf
}

/// Emit both the lean `EventStart` (event_id only — preserves the legacy
/// agent JSON contract) and the rich `EventDialog` (drives the HUD), and
/// queue the auto-end so an unattended session doesn't sit in an event
/// indefinitely. The auto-end is still wired the same way it was before
/// the C5 split — wiring user-driven event dismissal lands in C5 phase 2.
fn emit_event_dialog(
    event_tx: &broadcast::Sender<AgentEvent>,
    dialog: &crate::state::DialogState,
    pending_event_end: &mut Vec<(u32, u16, u16)>,
    name_cache: &std::collections::HashMap<u32, String>,
) {
    let _ = event_tx.send(AgentEvent::EventStart {
        event_id: dialog.event_id,
    });
    // Resolve from the session's id→name cache so off-screen NPCs (which
    // can fire events before their CHAR_NPC packet lands in the snapshot)
    // still surface a readable name. The HUD downstream further falls
    // back to `Entity.name` then to a hex placeholder.
    let mut dialog = dialog.clone();
    if dialog.npc_name.is_none() {
        dialog.npc_name = name_cache.get(&dialog.npc_id).cloned();
    }
    let _ = event_tx.send(AgentEvent::EventDialog {
        dialog: dialog.clone(),
    });
    // Use `event_para`, NOT `event_num`: per `0x032_event.cpp:42-43` the LSB
    // server writes `EventNum = PChar->getZone()` (zone id) and
    // `EventPara = eventInfo->eventId` (the real CSID). Our decoder labels
    // the offset-6-8 field `event_num` to match the protocol struct name,
    // but the value there is the zone, not the event id we need to echo
    // back in 0x05B EVENT_END. See the long comment on
    // `build_subpacket_event_end` for the field-label gotcha.
    pending_event_end.push((dialog.npc_id, dialog.act_index, dialog.event_para));
}

/// `GP_SERV_COMMAND_CHAT_STD` (0x017) decoder. Body layout per
/// `vendor/server/src/map/packets/s2c/0x017_chat_std.h`:
/// `Kind:u8 Attr:u8 Data:u16 sName[15] Mes[var]`. Returns `None` if the
/// body is shorter than the fixed prefix.
fn decode_chat_std(data: &[u8]) -> Option<ChatLine> {
    const PREFIX: usize = 4 + 15; // Kind + Attr + Data + sName
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

/// Decode the variable-length `Mes` field of a chat packet: walks the
/// NUL-terminated body and substitutes any inline auto-translate blocks
/// (`0xFD ty lang cat idx 0xFD`) with their resolved phrase. Falls back
/// to lossy UTF-8 for everything outside AT blocks. See
/// `ffxi_proto::autotranslate` for the lookup details.
fn decode_chat_text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    ffxi_proto::autotranslate::decode(&bytes[..end])
}

/// Decode a NUL-terminated, possibly NUL-padded byte slice as UTF-8 with
/// lossy fallback. Used for both `sName` and `Mes` fields in 0x017.
fn trim_nul_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
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
/// (size_words=5). `Mode=0`, `EndPara=choice` — choice 0 is the canonical
/// "skip whatever the NPC was trying to say" form; nonzero values pick a
/// branch in the event's lua-side `onEventFinish(choice)` handler.
///
/// **Field-label gotcha (load-bearing):** LSB's protocol struct names
/// `EventNum` and `EventPara` are reversed from how the values are used:
///
/// - `0x032 event.cpp:42-43` (server → client EVENTSTART) sets
///   `EventNum = zone_id` and `EventPara = eventInfo->eventId` (the real CSID).
/// - `0x05b_eventend.cpp:33,39` (client → server EVENT_END) reads
///   `this->EventPara` to validate against `PChar->currentEvent->eventId`
///   and to drive `OnEventFinish(eventId, result)`.
///
/// So **the cutscene/event id has to land in `EventPara` (offset 18)**, not
/// in `EventNum` (offset 16) where the misleading name suggests. We write
/// it into both — `EventPara` is what LSB reads; `EventNum` mirrors the
/// zone id we got back in the EVENTSTART so the wire looks symmetric with
/// the inbound packet (the real client sends both filled, per atom0s
/// reference linked in the LSB header). Prior to this fix every EVENT_END
/// we sent was silently rejected with "Event ID mismatch 535 != 0" because
/// `EventPara` stayed zero — the symptom is `/logout` continuing to fail
/// after `/endcutscene` "succeeds" client-side.
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
    // Mode u16 stays 0 (= End, per GP_CLI_COMMAND_EVENTEND_MODE::End)
    buf[16..18].copy_from_slice(&event_num.to_le_bytes());
    buf[18..20].copy_from_slice(&event_num.to_le_bytes());
    buf
}

/// `GP_CLI_COMMAND_ACTION` (0x01A) — 4-byte header + 4 UniqueNo + 2 ActIndex +
/// 2 ActionID + 16 ActionBuf = 28 bytes (size_words=7). The `ActionKind`
/// determines both the wire `ActionID` and the layout of the 16-byte buf
/// (see `ActionKind::fill_action_buf`).
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

/// `GP_CLI_COMMAND_ITEM_USE` (0x037) — 4-byte header + 4 UniqueNo + 4 ItemNum +
/// 2 ActIndex + 1 PropertyItemIndex + 1 padding00 + 4 Category = 20 bytes
/// (size_words=5). Per Phoenix/src/map/packets/c2s/0x037_item_use.cpp the
/// server validates `ItemNum == 0` and looks up the item via
/// `getStorage(Category)->GetItem(PropertyItemIndex)`, then resolves the
/// target via `PChar->GetEntity(ActIndex)` — so:
///
/// - `unique_no`  → wire `UniqueNo`     = recipient UniqueNo (self for potions).
/// - `act_index`  → wire `ActIndex`     = recipient ActIndex.
/// - `category`   → wire `Category`     = container id
///   (0 LOC_INVENTORY, 1 LOC_TEMPITEMS, …).
/// - `slot`       → wire `PropertyItemIndex` = slot index within the container.
/// - `ItemNum`    → forced to 0 (server enforces).
///
/// `GP_CLI_COMMAND_EQUIP_INSPECT` (0x0DD) — `/check` and friends. 4-byte
/// header + 4 UniqueNo + 4 ActIndex + 1 Kind + 3 padding = 16 bytes
/// (size_words=4). `kind` is `0=Check, 1=CheckName, 2=CheckParam` per
/// `vendor/server/src/map/packets/c2s/0x0dd_equip_inspect.h`. The server
/// replies with 0x0C9 (`equip_inspect_general` or
/// `equip_inspect_equipment`) once the inspect is resolved.
pub fn build_subpacket_equip_inspect(
    sync: u16,
    unique_no: u32,
    act_index: u16,
    kind: u8,
) -> Vec<u8> {
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(&build_subpacket_header(0x0DD, 4, sync));
    buf[4..8].copy_from_slice(&unique_no.to_le_bytes());
    // Wire ActIndex is u32, but our internal representation tops out at
    // u16 (ActIndex is bounded by zone entity count). Zero-extend.
    buf[8..12].copy_from_slice(&(act_index as u32).to_le_bytes());
    buf[12] = kind;
    // buf[13..16] is padding00, already zeroed.
    buf
}

/// `GP_CLI_COMMAND_REQLOGOUT` (0x0E7) — `/logout` / `/shutdown` request.
/// 4-byte header + 2 Mode + 2 Kind = 8 bytes (size_words=2). The server
/// validates `(Mode, Kind)` via `oneOf<...>` (see
/// `vendor/server/src/map/packets/c2s/0x0e7_reqlogout.cpp::validate`);
/// callers route through [`AgentCommand::ReqLogout`] +
/// [`crate::state::ReqLogoutKind`] which only emit pairs the server
/// accepts. Once the server's `EFFECT_LEAVEGAME` ticks down (≈30s for
/// normal players, immediate for GMs / Mog House per
/// `scripts/effects/leavegame.lua`), an s2c 0x00B `LOGOUT` lands and
/// the session loop's terminal-state path (state != ZONECHANGE) tears
/// down the connection.
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

/// `GP_CLI_COMMAND_CAMP` (0x0E8) — `/heal`. 4-byte sub-packet header +
/// 4-byte Mode (u32) = 8 bytes total (size_words=2). Layout:
/// `vendor/server/src/map/packets/c2s/0x0e8_camp.h`.
///
/// `mode` is a `u32` per the wire spec even though only three values
/// are valid (Toggle/On/Off); `HealMode::as_u32` provides them. Server
/// validation rejects the packet when engaged, in event, crafting,
/// abnormal status, or prevent-action — failures are silent on the wire
/// (no error message returned to the client).
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
    // buf[8..12] = ItemNum, must stay 0 — server-validated.
    buf[12..14].copy_from_slice(&act_index.to_le_bytes());
    buf[14] = slot;
    // buf[15] = padding00 stays 0
    buf[16..20].copy_from_slice(&(category as u32).to_le_bytes());
    buf
}

/// `GP_CLI_COMMAND_EQUIP_SET` (c2s 0x050) — equip one item from
/// inventory to a specific equipment slot. 4-byte sub-packet header +
/// 3 data bytes (PropertyItemIndex, EquipKind, Category) + 1 padding
/// byte to word-align = 8 bytes (size_words=2). Mirror of
/// `vendor/server/src/map/packets/c2s/0x050_equip_set.h`.
///
/// The s2c 0x050 EQUIP_LIST that comes back as confirmation has the
/// same three-field shape; Stage 1's decoder folds it into
/// `SessionState.equipment`, so a successful equip is visible in the
/// HUD on the next snapshot tick.
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
    // buf[7] padding stays 0
    buf
}

/// Emit a one-shot chat-system line on a false→true `MoghouseFlg`
/// transition for self. The opposite edge (true→false, mog-house exit) is
/// already obvious because entities reappear; the entry edge is the one
/// the operator misses, because the symptom is "no entities" with no other
/// chat signal. Idempotent: only fires on the rising edge.
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
        in_mog_house: attrs.moghouse_flg != 0,
    }
}

/// `GP_CLI_COMMAND_MAPRECT` (0x05E) — 4-byte header + RectID(4) + x/y/z(12) +
/// ActIndex(2) + MyRoomExitBit(1) + MyRoomExitMode(1) = 24 bytes
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

/// `GP_CLI_COMMAND_MAPRECT` (0x05E) built with `RectID="zmrq"` — the
/// universal "exit my Mog House" tag. Layout is identical to
/// [`build_subpacket_maprect`] but `RectID` is the four-byte ASCII tag and
/// the trailing `(MyRoomExitBit, MyRoomExitMode)` bytes carry the exit
/// selection (`MogHouseExit::wire_pair`). The server only honours these
/// trailing bytes when `RectID` matches the universal exit tag, so this
/// helper bakes that pairing in to make the precondition unforgeable at
/// the call site.
///
/// We sit on the existing pending-MAPRECT watchdog (`pending_maprect`) the
/// same way [`AgentCommand::RequestZoneChange`] does: if the server
/// silently rejects the exit (e.g. `mhflag` says the requested quest
/// zoneline isn't unlocked), the operator gets a chat-system line within
/// 3s explaining the most likely cause.
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
    // RectID = b"zmrq" as a little-endian u32. ASCII bytes are written in
    // file order so the four `char`s on the wire spell `z m r q`. The
    // server reads this as a `std::string_view` over the same bytes.
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

// =========================================================================
// 10 Hz Move cadence + self-reconciliation (rubber-band)
// =========================================================================

/// Minimum time between outbound POS subpacket emissions under sustained
/// motion. Matches retail's ~10 Hz Move cadence.
const MOVE_EMISSION_PERIOD: std::time::Duration = std::time::Duration::from_millis(100);

/// Position delta threshold that bypasses the 10 Hz rate-limit and forces
/// an immediate POS emission on the next keepalive tick. Sized below the
/// `effective_step_per_tick` (~0.165 yalm at base speed) × ~3 ticks so a
/// long-running integrator burst flushes promptly without spamming.
const MOVE_BIG_JUMP_YALMS: f32 = 0.5;

/// Self-reconciliation outcome when an inbound CHAR_PC for self carries a
/// position different from our local `self_pos`. The thresholds match
/// retail's "rubber-band" behavior — small deltas are ignored (client is
/// authoritative for sub-yalm jitter), medium deltas correct gradually,
/// and large deltas snap (zone teleport, GM warp).
#[derive(Debug, Clone, Copy, PartialEq)]
enum SelfPosReconcile {
    /// `delta <= 2.0 yalm` — local is trusted, server pos is ignored.
    KeepLocal,
    /// `2.0 < delta <= 10.0 yalm` — keep local now, lerp toward server
    /// at 5 yalm/s on subsequent ticks until the delta closes.
    Rubberband { target: Vec3 },
    /// `delta > 10.0 yalm` — snap to server pos immediately (zone change,
    /// teleport, etc.).
    Snap,
}

/// Decide what to do with an inbound server position for self.
/// `local` is what we believe; `server` is what the server just sent.
fn reconcile_self_pos(local: Vec3, server: Vec3) -> SelfPosReconcile {
    let dx = server.x - local.x;
    let dy = server.y - local.y;
    let dz = server.z - local.z;
    let dist_sq = dx * dx + dy * dy + dz * dz;
    // Squared comparisons to avoid the sqrt.
    if dist_sq <= 2.0 * 2.0 {
        SelfPosReconcile::KeepLocal
    } else if dist_sq <= 10.0 * 10.0 {
        SelfPosReconcile::Rubberband { target: server }
    } else {
        SelfPosReconcile::Snap
    }
}

/// Step `cur` toward `target` by at most `max_step` yalms in 3D.
/// Returns `(new_pos, reached)` — `reached` is true when the remaining
/// distance is consumed by this step (target reached this tick).
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

/// Gate for including a POS subpacket in the current keepalive bundle.
/// Returns true when ANY of the spec's three conditions hold:
///   1. `elapsed >= 100ms` since the last emission (10 Hz rate-limit).
///   2. `pos_delta > 0.5 yalm` (big jump — flush immediately).
///   3. heading byte changed (heading-only updates flush immediately).
fn should_emit_pos(
    elapsed: std::time::Duration,
    pos_delta_yalms: f32,
    heading_changed: bool,
) -> bool {
    elapsed >= MOVE_EMISSION_PERIOD || pos_delta_yalms > MOVE_BIG_JUMP_YALMS || heading_changed
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- CHAR_NPC entity classification (LSB live-mob marker) ----

    // A static targid (< 0x700) for the AH-counter / NPC cases; a dynamic
    // targid (0x700..=0x8FF) for spawned creatures.
    const STATIC_TARGID: u16 = 0x123;
    const DYNAMIC_TARGID: u16 = 0x712;

    #[test]
    fn standard_model_npc_is_not_a_mob() {
        // Auction House case: standard monster model (size 0), static targid,
        // UPDATE_HP present, but the live-mob marker (LSB body 0x25) is clear
        // because entity_update.cpp's TYPE_NPC branch never writes it. Must be
        // Npc so combat verbs aren't offered.
        assert_eq!(
            classify_char_npc(Some(0), STATIC_TARGID, false, true, false),
            EntityKind::Npc
        );
    }

    #[test]
    fn standard_model_live_mob_is_a_mob() {
        // Static instance mob: standard model, static targid, UPDATE_HP set,
        // marker set (hp>0) — resolved via the marker fallback.
        assert_eq!(
            classify_char_npc(Some(0), STATIC_TARGID, false, true, true),
            EntityKind::Mob
        );
    }

    #[test]
    fn dynamic_targid_mob_is_a_mob_without_marker() {
        // Regression: a field mob (Zeruhn Mines Leech) spawned with a dynamic
        // targid must be Mob even on a packet whose live-mob marker is clear
        // (no UPDATE_HP this tick). The targid range alone is authoritative.
        assert_eq!(
            classify_char_npc(Some(0), DYNAMIC_TARGID, false, false, false),
            EntityKind::Mob
        );
        assert_eq!(
            classify_char_npc(Some(0), DYNAMIC_TARGID, false, true, false),
            EntityKind::Mob
        );
    }

    #[test]
    fn standard_model_without_hp_update_defers_to_merge_kind() {
        // Static targid + no UPDATE_HP: can't read the marker, so emit Other
        // and let merge_kind keep any prior kind — never flip an established
        // NPC to Mob on a position-only tick.
        assert_eq!(
            classify_char_npc(Some(0), STATIC_TARGID, false, false, false),
            EntityKind::Other
        );
    }

    #[test]
    fn pc_owned_standard_model_is_a_pet() {
        // PMaster-is-a-PC wins over both the targid range and the mob marker
        // (jug pet / charmed mob).
        assert_eq!(
            classify_char_npc(Some(0), DYNAMIC_TARGID, true, true, true),
            EntityKind::Pet
        );
    }

    #[test]
    fn equipped_models_are_npcs_and_furniture_is_other() {
        assert_eq!(
            classify_char_npc(Some(1), STATIC_TARGID, false, true, false),
            EntityKind::Npc
        );
        assert_eq!(
            classify_char_npc(Some(7), STATIC_TARGID, false, false, false),
            EntityKind::Npc
        );
        for door_size in [2u16, 3, 4] {
            assert_eq!(
                classify_char_npc(Some(door_size), STATIC_TARGID, false, true, false),
                EntityKind::Other
            );
        }
        assert_eq!(
            classify_char_npc(None, STATIC_TARGID, false, true, true),
            EntityKind::Other
        );
    }

    // ---- 10 Hz Move cadence + rubber-band reconciliation ----

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    #[test]
    fn should_emit_pos_rate_limits_to_10hz() {
        // Below 100ms with no jump / heading change → suppress.
        assert!(!should_emit_pos(
            std::time::Duration::from_millis(50),
            0.1,
            false,
        ));
        // At/above 100ms → emit (10 Hz cadence under sustained motion).
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
        // >0.5 yalm delta forces emission even if <100ms elapsed.
        assert!(should_emit_pos(
            std::time::Duration::from_millis(10),
            0.6,
            false,
        ));
        // <=0.5 yalm at <100ms still suppressed.
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

    /// Cadence end-to-end: feed a synthetic 30 Hz integrator stream
    /// (33 ms apart, sub-yalm steps) and verify only ~10 ticks/s would
    /// emit a POS subpacket — i.e., the gate trips to ~3-tick spacing.
    #[test]
    fn cadence_drops_30hz_integrator_to_10hz_emission() {
        // Simulate 1 second of integrator output at 33 ms cadence with
        // ~0.165 yalm/tick (base run speed). The reactor never produces
        // a >0.5 yalm jump per tick, and heading stays constant, so only
        // the 100 ms rate-limit gates emission.
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
        // 1s / 100ms ≈ 10 expected. At 33 ms ticks, 100 ms gates align to
        // ~every 3rd tick → ~8-10 emissions/s in practice. The key
        // invariant is the drop from 30 → ~10, not the exact integer.
        assert!(
            (7..=11).contains(&emits),
            "expected ~10 emissions/s (10 Hz cadence vs 30 Hz integrator), got {emits}",
        );
    }

    #[test]
    fn reconcile_self_pos_keep_local_under_2_yalms() {
        // 1.5 yalms apart — within tolerance, trust local.
        let local = v(0.0, 0.0, 0.0);
        let server = v(1.0, 1.0, 0.5);
        assert_eq!(
            reconcile_self_pos(local, server),
            SelfPosReconcile::KeepLocal,
        );
    }

    #[test]
    fn reconcile_self_pos_rubberband_between_2_and_10() {
        // ~5 yalms apart — rubber-band, target = server pos.
        let local = v(0.0, 0.0, 0.0);
        let server = v(3.0, 4.0, 0.0); // distance 5
        match reconcile_self_pos(local, server) {
            SelfPosReconcile::Rubberband { target } => {
                assert_eq!(target, server);
            }
            other => panic!("expected Rubberband, got {other:?}"),
        }
    }

    #[test]
    fn reconcile_self_pos_snap_above_10_yalms() {
        // ~13 yalms apart — snap (zone teleport).
        let local = v(0.0, 0.0, 0.0);
        let server = v(12.0, 5.0, 0.0); // distance 13
        assert_eq!(reconcile_self_pos(local, server), SelfPosReconcile::Snap,);
    }

    #[test]
    fn reconcile_self_pos_boundaries() {
        // Exactly 2.0 yalms → KeepLocal (inclusive lower bound).
        let local = v(0.0, 0.0, 0.0);
        let just_inside = v(2.0, 0.0, 0.0);
        assert_eq!(
            reconcile_self_pos(local, just_inside),
            SelfPosReconcile::KeepLocal,
        );
        // Exactly 10.0 yalms → Rubberband (inclusive upper bound).
        let edge = v(10.0, 0.0, 0.0);
        assert!(matches!(
            reconcile_self_pos(local, edge),
            SelfPosReconcile::Rubberband { .. },
        ));
    }

    #[test]
    fn lerp_toward_advances_at_capped_step() {
        // 5 yalm step toward a 10-yalm-distant target should land
        // halfway, not at the target.
        let (next, reached) = lerp_toward(v(0.0, 0.0, 0.0), v(10.0, 0.0, 0.0), 5.0);
        assert!(!reached);
        assert!((next.x - 5.0).abs() < 1e-3);
    }

    #[test]
    fn lerp_toward_clamps_to_target_on_overshoot() {
        // Step bigger than distance → snap to target.
        let (next, reached) = lerp_toward(v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), 5.0);
        assert!(reached);
        assert_eq!(next, v(2.0, 0.0, 0.0));
    }

    /// Pin the load-bearing wire layout for 0x05B EVENT_END. The CSID
    /// (`event_num` arg) must land in **both** EventNum (offset 16) and
    /// EventPara (offset 18) — LSB validates against `EventPara`, and
    /// putting the value only in `EventNum` causes every EVENT_END to
    /// silently fail validation. See the long comment on
    /// `build_subpacket_event_end` for the field-label gotcha.
    #[test]
    fn event_end_writes_csid_to_event_para_field() {
        // Northern San d'Oria new-character cutscene: CSID=535, sent to
        // the player's own unique_no/act_index (forced cutscene). All
        // fields are little-endian.
        let buf = build_subpacket_event_end(
            0x1234,     // sync
            0xDEADBEEF, // unique_no
            0x4242,     // act_index
            535,        // event_num (the CSID per the arg semantics)
            0,          // choice
        );
        assert_eq!(buf.len(), 20, "header(4) + body(16)");

        // Body fields.
        assert_eq!(&buf[4..8], &0xDEADBEEFu32.to_le_bytes(), "UniqueNo");
        assert_eq!(&buf[8..12], &0u32.to_le_bytes(), "EndPara (choice=0)");
        assert_eq!(&buf[12..14], &0x4242u16.to_le_bytes(), "ActIndex");
        assert_eq!(&buf[14..16], &0u16.to_le_bytes(), "Mode (End=0)");

        // The fix: CSID must appear in EventPara (offset 18). LSB reads
        // *only* this field for validation.
        assert_eq!(
            &buf[18..20],
            &535u16.to_le_bytes(),
            "EventPara MUST carry the CSID — LSB validator reads from here",
        );
        // And mirrored in EventNum (offset 16) for symmetry with the
        // EVENTSTART the server sent.
        assert_eq!(
            &buf[16..18],
            &535u16.to_le_bytes(),
            "EventNum mirrors the CSID for atom0s wire symmetry",
        );
    }

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

    #[test]
    fn item_use_packet_layout_matches_phoenix_struct() {
        // GP_CLI_COMMAND_ITEM_USE body is:
        //   UniqueNo:u32 ItemNum:u32 ActIndex:u16 PropertyItemIndex:u8
        //   padding00:u8 Category:u32  → 16 body + 4 hdr = 20 total,
        // size_words = 5.
        let buf = build_subpacket_item_use(
            0xBEEF,     // sync
            0x12345678, // recipient UniqueNo (self)
            0x0042,     // recipient ActIndex
            0x00,       // category = LOC_INVENTORY
            7,          // slot
        );
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
        // 16-byte body. Hand-fill each field with a distinct value so the
        // mapping is obvious if any offset slips.
        let mut data = vec![0u8; 16];
        data[0..4].copy_from_slice(&0x1234_5678u32.to_le_bytes()); // UniqueNo
        data[4..6].copy_from_slice(&7u16.to_le_bytes()); // ActIndex
        data[6..8].copy_from_slice(&42u16.to_le_bytes()); // EventNum
        data[8..10].copy_from_slice(&3u16.to_le_bytes()); // EventPara
        data[10..12].copy_from_slice(&1u16.to_le_bytes()); // Mode
        data[12..14].copy_from_slice(&5u16.to_le_bytes()); // EventNum2
        data[14..16].copy_from_slice(&9u16.to_le_bytes()); // EventPara2

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
        // String[0] = "Selh"
        data[12..16].copy_from_slice(b"Selh");
        // String[1] = "Bastok"
        data[28..34].copy_from_slice(b"Bastok");
        // String[2,3] empty — should be trimmed from tail.
        // Data[0..2] = 100, 200
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
        // num[0] = -5, num[1] = 1234
        data[4..8].copy_from_slice(&(-5i32).to_le_bytes());
        data[8..12].copy_from_slice(&1234i32.to_le_bytes());
        // ActIndex/EventNum at the end.
        data[36..38].copy_from_slice(&3u16.to_le_bytes());
        data[38..40].copy_from_slice(&77u16.to_le_bytes());
        data[40..42].copy_from_slice(&2u16.to_le_bytes()); // EventPara
        data[42..44].copy_from_slice(&1u16.to_le_bytes()); // Mode

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
        // msg_basic id 1 is the canonical "<user> hits <target> for <amount>
        // points of damage." line. We hand-build a 0x029 body matching the
        // header struct, populate the cache for both ids, and assert the
        // substituted text.
        use std::collections::HashMap;

        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0x1111_1111u32.to_le_bytes()); // cas
        data[4..8].copy_from_slice(&0x2222_2222u32.to_le_bytes()); // tar
        data[8..12].copy_from_slice(&12u32.to_le_bytes()); // Data (amount)
        data[12..16].copy_from_slice(&0u32.to_le_bytes()); // Data2
        data[16..18].copy_from_slice(&3u16.to_le_bytes()); // ActIndexCas
        data[18..20].copy_from_slice(&4u16.to_le_bytes()); // ActIndexTar
        data[20..22].copy_from_slice(&1u16.to_le_bytes()); // MessageNum

        let mut cache = HashMap::new();
        cache.insert(0x1111_1111u32, "Sylvie".to_string());
        cache.insert(0x2222_2222u32, "Mandy".to_string());

        let line = decode_battle_message(&data, &cache, true).expect("decoded");
        assert_eq!(line.channel, ChatChannel::Battle);
        assert_eq!(line.sender, "Sylvie");
        assert!(line.text.contains("Sylvie"));
        assert!(line.text.contains("Mandy"));
        assert!(line.text.contains("12"));
    }

    #[test]
    fn battle_message_0x02d_uses_reordered_data_offsets() {
        // 0x02D moves Data/Data2 *after* the ActIndex pair. If the decoder
        // confused the two layouts, the amount substitution would pull
        // from the ActIndex slot and yield nonsense (a tiny number that
        // looks like an act_index, not an actual damage value).
        use std::collections::HashMap;

        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&1u32.to_le_bytes()); // cas
        data[4..8].copy_from_slice(&2u32.to_le_bytes()); // tar
        data[8..10].copy_from_slice(&7u16.to_le_bytes()); // ActIndexCas
        data[10..12].copy_from_slice(&8u16.to_le_bytes()); // ActIndexTar
        data[12..16].copy_from_slice(&999u32.to_le_bytes()); // Data (amount)
        data[16..20].copy_from_slice(&0u32.to_le_bytes()); // Data2
        data[20..22].copy_from_slice(&1u16.to_le_bytes()); // MessageNum=1

        let cache = HashMap::new();
        let line = decode_battle_message(&data, &cache, false).expect("decoded");
        assert!(
            line.text.contains("999"),
            "expected amount=999 from offsets [12..16], got: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_falls_back_to_hex_id_for_unknown_actor() {
        // A cas_id we haven't seen yet (e.g., a mob right after zone-in
        // before its CHAR_NPC arrives) must resolve to a hex token, not
        // panic and not silently drop the line.
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        data[4..8].copy_from_slice(&0u32.to_le_bytes()); // tar = 0 → "<no one>"
        data[8..12].copy_from_slice(&5u32.to_le_bytes());
        data[20..22].copy_from_slice(&1u16.to_le_bytes());
        let line = decode_battle_message(&data, &HashMap::new(), true).expect("decoded");
        assert_eq!(line.sender, "#DEADBEEF");
        assert!(line.text.contains("<no one>") || line.text.contains("#DEADBEEF"));
    }

    #[test]
    fn battle_message_97_routes_player_to_tar_and_target_to_cas() {
        // PlayerDefeatedBy = 97, template "<player> was defeated by the
        // <target>." Phoenix calls the constructor with `(PSender=killer,
        // PTarget=victim)` (charentity.cpp:2651), so on the wire:
        //   cas = killer (the orc)
        //   tar = victim (the player)
        // The template's `<player>` is the *victim* and `<target>` is the
        // *killer*, so the placeholder→slot binding is inverted vs. the
        // canonical "<user> hits <target>" pattern. Regression: prior to
        // the fix, `<player>` always pulled from `cas`, producing
        // "Orc was defeated by the player" — the swapped subject we saw
        // in production.
        use std::collections::HashMap;

        let killer_id = 0xAAAA_AAAAu32;
        let victim_id = 0xBBBB_BBBBu32;

        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&killer_id.to_le_bytes()); // cas = killer
        data[4..8].copy_from_slice(&victim_id.to_le_bytes()); // tar = victim
        data[20..22].copy_from_slice(&97u16.to_le_bytes());

        let mut cache = HashMap::new();
        cache.insert(killer_id, "Orcish_Fodder".to_string());
        cache.insert(victim_id, "Vanari".to_string());

        let line = decode_battle_message(&data, &cache, true).expect("decoded");
        // Subject (sender) of the chat row should be the victim.
        assert_eq!(line.sender, "Vanari");
        // Body should read "Vanari was defeated by the Orcish_Fodder."
        let v_pos = line.text.find("Vanari").expect("victim in text");
        let o_pos = line.text.find("Orcish_Fodder").expect("killer in text");
        assert!(
            v_pos < o_pos,
            "victim must precede killer in the rendered template, got: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_8_exp_gain_substitutes_hash_marker() {
        // ExperiencePointsGained = 8, "<player> gains # experience points."
        // Emitted via BATTLE_MESSAGE2 (0x02D), so Data sits at offsets 12/16.
        // Regression: the dispatcher previously inverted is_029, pulling the
        // amount from the ActIndex slot and leaving `#` literal.
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes()); // cas
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes()); // tar (same — self)
        data[12..16].copy_from_slice(&420u32.to_le_bytes()); // Data = exp
        data[16..20].copy_from_slice(&0u32.to_le_bytes()); // Data2
        data[20..22].copy_from_slice(&8u16.to_le_bytes()); // MessageNum
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "hello".to_string());
        // is_029=false matches the 0x02D layout.
        let line = decode_battle_message(&data, &cache, false).expect("decoded");
        assert!(
            line.text.contains("420") && !line.text.contains('#'),
            "expected '#' to be replaced with 420, got: {}",
            line.text
        );
        assert!(line.text.contains("hello"));
    }

    #[test]
    fn battle_message_38_skill_gain_substitutes_skill_and_x() {
        // SkillGain = 38, "<target>'s <skill> skill rises X points."
        // Data = SkillID, Data2 = points. Emitted via 0x029 (is_029=true).
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes()); // cas
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes()); // tar
        data[8..12].copy_from_slice(&48u32.to_le_bytes()); // Data = SKILL_FISHING
        data[12..16].copy_from_slice(&3u32.to_le_bytes()); // Data2 = 3 points
        data[20..22].copy_from_slice(&38u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "hello".to_string());
        let line = decode_battle_message(&data, &cache, true).expect("decoded");
        // Retail divides the raw amount by 10 for display: 3 → "0.3".
        assert!(
            line.text.contains("Fishing") && line.text.contains("rises 0.3 points"),
            "expected '<skill>'→Fishing and 'X'→0.3 (decimal), got: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_53_skill_level_up_renders_x_as_integer() {
        // SkillLevelUp = 53. Server already divides by 10 (charutils.cpp:4161
        // sends `(CurSkill + SkillAmount) / 10`), so X must NOT be re-divided
        // here. Template: "<target>'s <skill> skill reaches level X."
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[8..12].copy_from_slice(&1u32.to_le_bytes()); // SKILL_HAND_TO_HAND
        data[12..16].copy_from_slice(&12u32.to_le_bytes()); // level = 12
        data[20..22].copy_from_slice(&53u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "hello".to_string());
        let line = decode_battle_message(&data, &cache, true).expect("decoded");
        assert!(
            line.text.contains("level 12") && !line.text.contains("1.2"),
            "expected integer level, got: {}",
            line.text
        );
    }

    #[test]
    fn battle_message_253_exp_chain_substitutes_two_hashes_in_order() {
        // ExpChain = 253, "EXP chain #! <player> gains # experience points."
        // Data = exp, Data2 = chainNumber. First `#` is data2 (chain),
        // second `#` is data1 (exp). Via BATTLE_MESSAGE2 (is_029=false).
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[12..16].copy_from_slice(&320u32.to_le_bytes()); // Data = exp
        data[16..20].copy_from_slice(&5u32.to_le_bytes()); // Data2 = chain #5
        data[20..22].copy_from_slice(&253u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "hello".to_string());
        let line = decode_battle_message(&data, &cache, false).expect("decoded");
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
        // A bare `X` in a real msg_basic template should be swapped;
        // a within-word X (like the 'X' in "BoXing" — unlikely in real
        // templates but worth pinning the rule) must NOT be swapped.
        // Use msg id 53 (SkillLevelUp, integer formatting) so the
        // decimal-tenths special case for SkillGain (38) / SkillDrop
        // (310) doesn't muddy the assertion.
        let s = substitute_battle_placeholders(
            "reaches level X. BoXing.",
            "cas",
            "tar",
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
        // MagicDamage = 2, "<caster> casts <spell>. <target> takes <amount>
        // points of damage." With ffxi_proto::spell_names wired, spell 144
        // resolves to "Fire" — not the legacy "spell #144" fallback.
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes()); // cas (caster)
        data[4..8].copy_from_slice(&0xBEEFu32.to_le_bytes()); // tar
        data[8..12].copy_from_slice(&144u32.to_le_bytes()); // Data = SpellID = Fire
        data[12..16].copy_from_slice(&0u32.to_le_bytes());
        data[20..22].copy_from_slice(&2u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        cache.insert(0xBEEFu32, "Mandragora".to_string());
        let line = decode_battle_message(&data, &cache, true).expect("decoded");
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
        // MsgBasic::Obtains (565). LSB header comment is
        // "<target> obtains <amount>." but every LSB call site sends
        // this for gil distribution (charutils.cpp:4756, 4763), so the
        // TEMPLATE_OVERRIDES table swaps in "<target> obtains <amount> gil."
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes()); // cas
        data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes()); // tar (self)
        data[8..12].copy_from_slice(&4u32.to_le_bytes()); // Data = gil amount
        data[12..16].copy_from_slice(&0u32.to_le_bytes());
        data[20..22].copy_from_slice(&565u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Mithy".to_string());
        let line = decode_battle_message(&data, &cache, true).expect("decoded");
        assert_eq!(line.text, "Mithy obtains 4 gil.", "got: {}", line.text);
    }

    #[test]
    fn substitute_status_placeholder_resolves_effect_name() {
        // Direct substitution test: <status> resolves via
        // ffxi_proto::status_names::lookup(data1).
        let s = substitute_battle_placeholders(
            "gains the effect of <status>.",
            "cas",
            "tar",
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
        // ReadiesWeaponskill = 43, "<entity> readies <skill>." The actor
        // ships in slot Cas; before this change `<entity>` wasn't in our
        // substitution list and rendered literally.
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
        data[4..8].copy_from_slice(&0u32.to_le_bytes());
        data[8..12].copy_from_slice(&1u32.to_le_bytes()); // SkillID = Hand-to-Hand
        data[12..16].copy_from_slice(&0u32.to_le_bytes());
        data[20..22].copy_from_slice(&43u16.to_le_bytes());
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        let line = decode_battle_message(&data, &cache, true).expect("decoded");
        assert!(
            line.text.contains("Daisy readies Hand-to-Hand") && !line.text.contains("<entity>"),
            "expected '<entity>' → Daisy, got: {}",
            line.text
        );
    }

    /// Test helper: bit writer mirroring LSB's `packBitsBE`
    /// (`vendor/server/src/common/utils.cpp:272`). Stores multi-byte
    /// fields in native little-endian byte order; within a byte, the
    /// low bits hold the first-packed field. See [`BattleBitReader`]'s
    /// doc comment for the full convention.
    ///
    /// `start_bit = 8` matches the body shape `walk_sub_packets` hands
    /// to `decode_battle2_action` (1-byte workSize + bitstream).
    struct BattleBitWriter {
        data: Vec<u8>,
        pos: usize,
    }

    impl BattleBitWriter {
        fn new(start_bit: usize) -> Self {
            // 1 KB is plenty for a few targets × few results.
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
        // One target, one result: AttackHits (msg id 1), 42 damage.
        // The decoder must emit one ChatLine reading
        //   "Daisy hits Mandragora for 42 points of damage."
        use std::collections::HashMap;
        let mut w = BattleBitWriter::new(8);
        w.write(0xCAFEu64, 32); // actor_id (caster)
        w.write(1, 6); // trg_sum = 1
        w.write(0, 4); // res_sum (unused)
        w.write(0, 4); // cmd_no
        w.write(0, 32); // cmd_arg
        w.write(0, 32); // info
                        // Target 0:
        w.write(0xBEEFu64, 32); // target_id
        w.write(1, 4); // result_sum = 1
                       // Result 0:
        w.write(0, 3); // miss/resolution
        w.write(0, 2); // kind
        w.write(0, 12); // sub_kind (animation)
        w.write(0, 5); // info
        w.write(0, 5); // scale
        w.write(42, 17); // value (damage)
        w.write(1, 10); // message = AttackHits
        w.write(0, 31); // modifier
        w.write(0, 1); // has_proc = false
        w.write(0, 1); // has_react = false

        let data = w.into_bytes();
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        cache.insert(0xBEEFu32, "Mandragora".to_string());

        let lines = decode_battle2_action(&data, &cache);
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
        // MagicDamage (msg 2): "<caster> casts <spell>. <target> takes
        // <amount> points of damage." cmd_arg = SpellID = 144 (Fire),
        // result.value = 87 damage. The decoder must thread cmd_arg
        // through `substitute_battle_placeholders` as the action_id
        // so `<spell>` resolves to "Fire" via ffxi_proto::spell_names.
        use std::collections::HashMap;
        let mut w = BattleBitWriter::new(8);
        w.write(0xCAFE, 32);
        w.write(1, 6); // trg_sum
        w.write(0, 4); // res_sum
        w.write(4, 4); // cmd_no = spell
        w.write(144, 32); // cmd_arg = SpellID
        w.write(0, 32);
        w.write(0xBEEF, 32);
        w.write(1, 4);
        w.write(0, 3);
        w.write(0, 2);
        w.write(0, 12);
        w.write(0, 5);
        w.write(0, 5);
        w.write(87, 17); // damage
        w.write(2, 10); // message = MagicDamage
        w.write(0, 31);
        w.write(0, 1);
        w.write(0, 1);

        let data = w.into_bytes();
        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        cache.insert(0xBEEFu32, "Mandragora".to_string());

        let lines = decode_battle2_action(&data, &cache);
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert!(
            l.text.contains("Daisy") && l.text.contains("Fire") && l.text.contains("87"),
            "expected casts/Fire/87 in: {}",
            l.text
        );
    }

    #[test]
    fn battle2_drops_results_with_zero_message_id() {
        // A result with message_num=0 means "no message" (animation-
        // only frame). Must NOT emit a chat line — would render as a
        // blank or wrong-template row otherwise.
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
        w.write(0, 10); // message = 0 (none)
        w.write(0, 31);
        w.write(0, 1);
        w.write(0, 1);
        let data = w.into_bytes();
        let lines = decode_battle2_action(&data, &HashMap::new());
        assert!(lines.is_empty(), "expected drop, got: {:?}", lines);
    }

    #[test]
    fn battle2_bitwriter_matches_lsb_pack_byte_layout() {
        // Pin LSB's packBitsBE convention with a known value at a known
        // offset, asserting the *byte representation* directly. This is
        // independent of BattleBitReader — if a future drive-by edit
        // flips both reader and writer to MSB-first they'd still mirror
        // each other but the live wire format would break silently.
        // This test catches that by checking the bytes themselves.
        //
        // Layout per vendor/server/src/common/utils.cpp:272:
        //   actor_id = 0xCAFE at bit 8 (byte 1, bit-in-byte 0)
        //   → write 32-bit LE u32 to target[1..5]
        //   → target[1] = 0xFE, target[2] = 0xCA, target[3..5] = 0
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
        // Wire-shape regression. `walk_sub_packets` strips the 4-byte
        // sub-packet header (see ffxi-proto/src/framing.rs:157
        // `&self.rest[4..size_bytes]`), so what reaches
        // `decode_battle2_action` is `[workSize:u8][bitstream...]` —
        // bit offset 8, not 40.
        //
        // LSB's pack/unpack starts at `8 * 5 = 40` because it operates
        // on the full buffer including the 4-byte header
        // (vendor/server/src/map/packets/s2c/0x028_battle2.cpp:37,120).
        // A drive-by edit that "syncs" our start-bit back to 40 would
        // skip 4 extra bytes of real payload and silently drop every
        // combat action.
        //
        // This test pins the convention: synthesize the exact shape
        // `walk_sub_packets` produces and assert one well-formed line
        // emerges. The workSize byte is computed the same way LSB does
        // it (`(bitOffset >> 3) + (bitOffset % 8 != 0)`) so future
        // diffs that take workSize semantics seriously also have a
        // reference value.
        use std::collections::HashMap;
        let mut w = BattleBitWriter::new(8);
        w.write(0xCAFE, 32);
        w.write(1, 6); // trg_sum
        w.write(0, 4); // res_sum
        w.write(0, 4); // cmd_no
        w.write(0, 32); // cmd_arg
        w.write(0, 32); // info
        w.write(0xBEEF, 32);
        w.write(1, 4);
        w.write(0, 3);
        w.write(0, 2);
        w.write(0, 12);
        w.write(0, 5);
        w.write(0, 5);
        w.write(42, 17); // damage
        w.write(1, 10); // message = AttackHits
        w.write(0, 31);
        w.write(0, 1);
        w.write(0, 1);

        let mut data = w.into_bytes();
        // workSize = byte count of the bitstream portion (excludes the
        // workSize byte itself). The bitstream begins at bit 8 and ends
        // at the current writer position; round up to bytes.
        let bitstream_bits = data.len() * 8 - 8;
        data[0] = bitstream_bits.div_ceil(8) as u8;

        let mut cache = HashMap::new();
        cache.insert(0xCAFEu32, "Daisy".to_string());
        cache.insert(0xBEEFu32, "Mandragora".to_string());

        let lines = decode_battle2_action(&data, &cache);
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
        // Message id 0xFFFF isn't in msg_basic (and won't be — it's a
        // sentinel). The decoder must return None so the dispatcher
        // skips emitting an empty chat line.
        use std::collections::HashMap;
        let mut data = vec![0u8; 24];
        data[20..22].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert!(decode_battle_message(&data, &HashMap::new(), true).is_none());
    }

    /// Build a 0x029 BattleMessage body for a synth-decoded id (Check
    /// or Checkparam family) so the helper paths can be exercised
    /// end-to-end through `decode_battle_message`.
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
        // /check on a mob with even def/eva (message_num=174), data1=53
        // (level), data2=64+4=68 (Even Match). No def/eva suffix on
        // even/even.
        use std::collections::HashMap;
        let data = check_message(174, 53, 64 + 4, 1, 2);
        let mut cache = HashMap::new();
        cache.insert(1u32, "Daisy".to_string());
        cache.insert(2u32, "Goblin".to_string());

        let line = decode_battle_message(&data, &cache, true).expect("decoded");
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
        // 9 ids, each tied to one (def, eva) modifier pair via LSB's
        // calc (`0x0dd_equip_inspect.cpp:99-121`): defense is +/-1,
        // evasion is +/-3. Each id must produce the matching English
        // phrase.
        use std::collections::HashMap;
        let cache: HashMap<u32, String> = [(2u32, "Mob".to_string())].into_iter().collect();
        let cases: &[(u16, Option<&str>, Option<&str>)] = &[
            // total = -4: high def + high eva
            (170, Some("high defense"), Some("high evasion")),
            // total = -3: even def + high eva
            (171, None, Some("high evasion")),
            // total = -2: low def + high eva
            (172, Some("low defense"), Some("high evasion")),
            // total = -1: high def + even eva
            (173, Some("high defense"), None),
            // total = 0: even/even
            (174, None, None),
            // total = +1: low def + even eva
            (175, Some("low defense"), None),
            // total = +2: high def + low eva
            (176, Some("high defense"), Some("low evasion")),
            // total = +3: even def + low eva
            (177, None, Some("low evasion")),
            // total = +4: low def + low eva
            (178, Some("low defense"), Some("low evasion")),
        ];
        for &(msg, def_phrase, eva_phrase) in cases {
            let data = check_message(msg, 25, 64 + 3, 1, 2); // Lv 25, Decent Challenge
            let line = decode_battle_message(&data, &cache, true).expect("decoded");
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
        // `data2 - 64` is the EMobDifficulty enum. All 8 tiers must
        // produce a distinct, non-fallback label.
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
            let line = decode_battle_message(&data, &cache, true).expect("decoded");
            assert!(
                line.text.contains(expected),
                "tier {tier} expected {expected:?}: {}",
                line.text
            );
        }
    }

    #[test]
    fn checkparam_renders_acc_att_pairs() {
        // /checkparam pushes 712/713/714/715 with (ACC, ATT)-ish
        // numeric fields. Render must surface both numbers.
        use std::collections::HashMap;
        let cache: HashMap<u32, String> = [(1u32, "Daisy".to_string())].into_iter().collect();
        for (msg, label) in [
            (712u16, "Main weapon"),
            (713, "Auxiliary weapon"),
            (714, "Ranged weapon"),
            (715, "Evasion"),
        ] {
            let data = check_message(msg, 321, 654, 1, 1);
            let line = decode_battle_message(&data, &cache, true).expect("decoded");
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
        // LSB sends (0, 0) for an unequipped sub/ranged slot. The
        // operator-facing line must read as "none equipped" rather than
        // "Accuracy: 0, Attack: 0".
        use std::collections::HashMap;
        let cache: HashMap<u32, String> = [(1u32, "Daisy".to_string())].into_iter().collect();
        for msg in [713u16, 714] {
            let data = check_message(msg, 0, 0, 1, 1);
            let line = decode_battle_message(&data, &cache, true).expect("decoded");
            assert!(
                line.text.to_ascii_lowercase().contains("none equipped"),
                "msg {msg} with (0,0) should read \"none equipped\", got: {}",
                line.text
            );
        }
    }

    #[test]
    fn check_impossible_to_gauge_uses_mob_placeholder() {
        // Id 249 is `CheckImpossibleToGauge`; LSB sends it with the mob
        // entity in the Tar slot. `<mob>` placeholder must resolve to
        // the target name.
        use std::collections::HashMap;
        let data = check_message(249, 0, 0, 1, 2);
        let mut cache = HashMap::new();
        cache.insert(1u32, "Daisy".to_string());
        cache.insert(2u32, "King Behemoth".to_string());

        let line = decode_battle_message(&data, &cache, true).expect("decoded");
        assert!(
            line.text.contains("King Behemoth")
                && line.text.to_ascii_lowercase().contains("impossible"),
            "{}",
            line.text
        );
    }

    #[test]
    fn miscdata_status_icons_drops_placeholder_slots() {
        // Body layout: u16 type, u16 unknown06, 32×u16 icons, 32×u32 timestamps.
        let mut data = vec![0u8; 4 + 64 + 128];
        data[0..2].copy_from_slice(&0x0009u16.to_le_bytes()); // type=StatusIcons
                                                              // Slot 0: real icon 33
        data[4..6].copy_from_slice(&33u16.to_le_bytes());
        // Slot 1: placeholder 0x00FF — must be dropped.
        data[6..8].copy_from_slice(&0x00FFu16.to_le_bytes());
        // Slot 2: real icon 12
        data[8..10].copy_from_slice(&12u16.to_le_bytes());
        // Remaining slots stay 0 — also dropped.
        let icons = decode_miscdata_status_icons(&data).expect("decoded");
        assert_eq!(icons, vec![33, 12]);
    }

    #[test]
    fn miscdata_status_icons_rejects_wrong_type() {
        let mut data = vec![0u8; 4 + 64 + 128];
        data[0..2].copy_from_slice(&0x0005u16.to_le_bytes()); // JobPoints, not StatusIcons
                                                              // Even with valid icon bytes, the type guard should bail.
        data[4..6].copy_from_slice(&33u16.to_le_bytes());
        assert!(decode_miscdata_status_icons(&data).is_none());
    }

    #[test]
    fn miscdata_status_icons_truncated_returns_none() {
        let data = vec![0u8; 10]; // way under the icons array
        assert!(decode_miscdata_status_icons(&data).is_none());
    }

    #[test]
    fn shop_list_decodes_rows_and_skips_zero_padding() {
        // Two real rows + one zeroed-out tail row. Decoder must keep the
        // two real ones and drop the padding row.
        let mut data = vec![0u8; 4 + 12 * 3];
        data[0..2].copy_from_slice(&5u16.to_le_bytes()); // ShopItemOffsetIndex

        // Row 0 at offset 4: price=100, item=4096, idx=0
        data[4..8].copy_from_slice(&100u32.to_le_bytes());
        data[8..10].copy_from_slice(&4096u16.to_le_bytes());
        data[10] = 0; // shop_index
                      // Row 1 at offset 16: price=99999, item=256, idx=1
        data[16..20].copy_from_slice(&99999u32.to_le_bytes());
        data[20..22].copy_from_slice(&256u16.to_le_bytes());
        data[22] = 1; // shop_index
                      // Row 2 at offset 28: zeroed (item_no = 0) → must be skipped.

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
        // Layout per vendor/server/src/map/packets/c2s/0x083_shop_buy.h:
        //   u32 ItemNum, u16 ShopNo, u16 ShopItemIndex,
        //   u8 PropertyItemIndex, u8 pad[3]
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
        // Layout per vendor/server/src/map/packets/c2s/0x0e8_camp.h:
        //   uint32_t Mode    // 0=Toggle, 1=On, 2=Off
        // Sub-packet header (4 bytes) is 0x0E8 opcode + size_words=2 + sync.
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
    fn equip_inspect_packet_layout_matches_server_struct() {
        // Layout per vendor/server/src/map/packets/c2s/0x0dd_equip_inspect.h:
        //   u32 UniqueNo, u32 ActIndex, u8 Kind, u8 padding00[3]
        // Sub-packet header (4 bytes) is 0x0DD opcode + size_words=4 + sync.
        let buf = build_subpacket_equip_inspect(0xABCD, 0x1234_5678, 42, 1);
        assert_eq!(buf.len(), 16, "header (4) + body (12)");
        // Header opcode + size_words. Header layout: opcode | (size_words << 9).
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
        // Sanity: a higher category id (8 = LOC_WARDROBE) lands in all
        // four bytes of the Category field, not just the low byte.
        let buf = build_subpacket_item_use(0, 0, 0, 8, 0);
        assert_eq!(
            u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            8,
            "Category u32 LE"
        );
    }

    /// Hand-build a 0x017 sub-packet body and verify the decoder maps each
    /// `CHAT_MESSAGE_TYPE` byte to the expected `ChatChannel`. The kinds we
    /// surface with full fidelity are the ones the chat panel filters on;
    /// the rest collapse to `Other`.
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
        // Kind = SAY, sName = "Sylvie", Mes = "hi all". `sName` lives at
        // body offset 4..19 (15 bytes, NUL-padded).
        let mut body = vec![0u8; 4 + 15];
        body[0] = ffxi_proto::map::chat_kind::SAY;
        body[4..10].copy_from_slice(b"Sylvie");
        // sName[10..19] stays zero
        body.extend_from_slice(b"hi all");
        body.push(0);
        let line = decode_chat_std(&body).unwrap();
        assert_eq!(line.sender, "Sylvie");
        assert_eq!(line.text, "hi all");
        assert_eq!(line.channel, ChatChannel::Say);
    }

    #[test]
    fn chat_std_decoder_rejects_truncated_body() {
        // Anything shorter than the fixed prefix (Kind+Attr+Data+sName = 19)
        // is unparseable — return None instead of panicking.
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
        // EXECUTING_LOGOUT (id=7) — the /logout countdown ticker. The
        // scraped text from `xi.msg.system` should land here verbatim
        // with `<seconds>` filled from para.
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
