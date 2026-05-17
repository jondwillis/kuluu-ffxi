//! Session state — the single source of truth that both the TUI and the
//! JSON sidechannel subscribe to.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Process-monotonic clock: ms elapsed since the first call. Used to stamp
/// `LlmDecision.at_monotonic_ms` (producer) and to compute pulse-decay age
/// at render time (consumer). Both sides must share this anchor or the
/// "ms since most recent decision" math goes negative.
///
/// The anchor lazy-inits on first call. In ffxi-mcp the supervisor and
/// notifier both call this early, so by the time chrome renders the
/// anchor is guaranteed set.
pub fn process_monotonic_ms() -> u64 {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    let start = ANCHOR.get_or_init(Instant::now);
    start.elapsed().as_millis() as u64
}

/// Stage of the end-to-end login flow we're currently in.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Idle,
    Authenticating,
    LobbyHandshake,
    MapBootstrap,
    Zoning,
    InZone,
    Disconnected,
}

/// FFXI Blowfish lifecycle, mirrored from `server/src/common/blowfish.h::BLOWFISH`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlowfishStatus {
    Waiting,
    Sent,
    Accepted,
    PendingZone,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Position {
    pub pos: Vec3,
    /// 0..=255 mapping to 0°..360°, matches `GP_CLI_COMMAND_POS::dir`.
    pub heading: u8,
    /// Current effective movement speed (server-set). FFXI's typical PC base
    /// is 25 → 5 yalms/sec; modifiers (Bind/Quickening/etc.) scale this.
    /// Source byte: `PosHead::speed` in the server `0x00D` packet. Not
    /// populated from packets today — all writers leave it at the
    /// [`Default`] of 25 — but the field is here so wire consumers
    /// (relay/wasm/native viewer) can read it once the decoder threads it
    /// through.
    #[serde(default = "default_speed")]
    pub speed: u8,
    /// Unmodified base speed. Same caveat as `speed`.
    #[serde(default = "default_speed")]
    pub speed_base: u8,
}

fn default_speed() -> u8 {
    25
}

fn default_fps() -> u32 {
    60
}

impl Default for Position {
    fn default() -> Self {
        Self {
            pos: Vec3::default(),
            heading: 0,
            speed: default_speed(),
            speed_base: default_speed(),
        }
    }
}

/// Compute (dx, dy) for "1 unit forward at heading h" in our horizontal
/// plane. FFXI heading is u8 where, matching LSB's `worldAngle`
/// (vendor/server/src/common/utils.cpp:130-140) and our `heading_toward`,
/// heading 0 = +x (east), 64 = south, 128 = west, 192 = north — CW
/// viewed from above, wrapping at 256 = 360°. The byte's semantic is what
/// the server stores in `loc.p.rotation`; mirroring that convention here
/// keeps the 2D minimap, the 3D view, and the wire packets all reading
/// the same byte the same way.
#[inline]
pub fn heading_to_forward(heading: u8) -> (f32, f32) {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    (angle.cos(), -angle.sin())
}

/// Cycle to the next nearby entity by 2D (xy-plane) distance from `from`.
/// Identifies the target by `Entity::id` rather than slice index because
/// the entity list reshuffles between snapshots — an index from one frame
/// would cycle to the wrong entity (or panic) on the next.
///
/// Both renderers use this so Tab semantics stay identical.
pub fn next_target_by_distance(
    entities: &[Entity],
    from: Vec3,
    current: Option<u32>,
) -> Option<u32> {
    if entities.is_empty() {
        return None;
    }
    let mut order: Vec<(&Entity, f32)> = entities
        .iter()
        .map(|e| {
            let dx = e.pos.x - from.x;
            let dy = e.pos.y - from.y;
            (e, dx * dx + dy * dy)
        })
        .collect();
    order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let ids: Vec<u32> = order.iter().map(|(e, _)| e.id).collect();
    match current.and_then(|id| ids.iter().position(|&i| i == id)) {
        Some(p) => Some(ids[(p + 1) % ids.len()]),
        None => Some(ids[0]),
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Pc,
    Npc,
    Mob,
    Pet,
    Other,
}

/// Merge two observations of an entity's kind. `Other` is the "unknown"
/// bucket — once a specialized classification (`Pc`/`Npc`/`Mob`/`Pet`) has
/// been established for an entity, a follow-up `Other` should not demote it.
/// Among specialized kinds the newer observation wins.
fn merge_kind(existing: EntityKind, incoming: EntityKind) -> EntityKind {
    use EntityKind::*;
    match (existing, incoming) {
        (Pc | Npc | Mob | Pet, Other) => existing,
        _ => incoming,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: u32,
    /// Per-zone short index (the `ActIndex` server-side). Required when
    /// targeting via `0x01A ACTION` — `id` alone is not enough; the server
    /// looks the entity up by `ActIndex` within the current zone.
    pub act_index: u16,
    pub kind: EntityKind,
    pub name: Option<String>,
    pub pos: Vec3,
    pub heading: u8,
    /// HP percentage, 0..=100. None if unknown.
    pub hp_pct: Option<u8>,
    /// `BtTargetID` from `GP_SERV_POS_HEAD`. Zero = not in combat.
    /// Drives `EngagedBy` aggro detection in the reactor.
    #[serde(default)]
    pub bt_target_id: u32,
    /// `m_OwnerID` for mobs (the canonical FFXI claim id). The byte slot
    /// shared with `BtTargetID` in the wire layout is repurposed for mobs
    /// in the CHAR_NPC packet (`entity_update.cpp::updateWith`). `0` for
    /// PCs/NPCs and unclaimed mobs. Drives mob capsule color in the
    /// viewer.
    #[serde(default)]
    pub claim_id: u32,
    /// Current movement speed (`PosHead::speed`). 0 when standing still.
    /// Used by the local WASD integrator to compute step size per tick,
    /// and by Option C (smooth wire-side movement) to set `MoveFlame`.
    #[serde(default)]
    pub speed: u8,
    /// Base movement speed (`PosHead::speed_base`) — animation speed,
    /// unaffected by movement-status effects. Kept alongside `speed` so
    /// renderers can pick the right animation play rate.
    #[serde(default)]
    pub speed_base: u8,
    /// Decoded model selector from CHAR_NPC / CHAR_PC (LSB's
    /// `MODELTYPE`). Drives MMB lookup in the viewer; `None` until a
    /// look-bearing packet for this entity has been received. Carried
    /// as `ffxi_proto::decode::LookData` (no serde) — the wire-side
    /// `EntityLook` mirror gets built in `wire_translate` so this
    /// crate doesn't need a feature-gated viewer-wire dep here.
    #[serde(skip)]
    pub look: Option<ffxi_proto::decode::LookData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLine {
    pub channel: ChatChannel,
    pub sender: String,
    pub text: String,
    /// Vana'diel (server) timestamp in seconds since epoch.
    pub server_ts: u32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatChannel {
    Say,
    Shout,
    Tell,
    Party,
    Linkshell,
    Yell,
    System,
    Other,
    /// Combat-log line: substituted text from `ffxi_proto::msg_basic`
    /// driven by 0x029 / 0x02D battle messages. Translated to
    /// `wire::ChatChannel::Battle` and rendered in orange.
    Battle,
}

impl ChatChannel {
    /// Map the wire `CHAT_MESSAGE_TYPE` byte (0x017's first body byte, also
    /// the kind echoed back in our own outgoing 0x0B5) onto the operator-
    /// facing channel taxonomy. Unknown kinds collapse to `Other` so we
    /// don't lose visibility on novel server messages — they just stack in
    /// the catch-all bucket until a real channel is added.
    pub fn from_chat_kind(kind: u8) -> Self {
        use ffxi_proto::map::chat_kind as k;
        match kind {
            k::SAY | k::NS_SAY => Self::Say,
            k::SHOUT | k::NS_SHOUT => Self::Shout,
            k::TELL => Self::Tell,
            k::PARTY | k::NS_PARTY => Self::Party,
            k::LINKSHELL | k::NS_LINKSHELL | k::LINKSHELL2 | k::NS_LINKSHELL2 => Self::Linkshell,
            k::YELL => Self::Yell,
            k::SYSTEM_1 | k::SYSTEM_2 | k::SYSTEM_3 => Self::System,
            // Emotes render alongside Say in retail; same channel keeps them
            // visually grouped without inventing a new variant.
            k::EMOTION => Self::Say,
            _ => Self::Other,
        }
    }
}

/// Per-connection diagnostics surfaced to humans (TUI footer) and agents
/// (JSON sidechannel). These are the *first-class* signals an agent needs to
/// know the session is healthy — surfacing sync_in/sync_out is what catches
/// silent sequence desync (failure mode #1 in the plan).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub stage: Option<Stage>,
    pub blowfish_status: Option<BlowfishStatus>,
    pub sync_in: Option<u16>,
    pub sync_out: Option<u16>,
    /// Milliseconds since we last received a server bundle.
    pub last_server_packet_age_ms: Option<u64>,
    /// SHA-256 of the auth-server TLS cert, hex-encoded.
    pub cert_sha256: Option<String>,
    pub map_server_addr: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub stage: Stage,
    pub account_id: Option<u32>,
    pub char_id: Option<u32>,
    pub character: Option<String>,
    pub zone_id: Option<u16>,
    pub entities: Vec<Entity>,
    pub party: Vec<PartyMember>,
    pub chat: Vec<ChatLine>,
    pub diagnostics: Diagnostics,
    /// Mirrored inventory state. Populated by the Stage-9 0x01C/D/E/F/0x020
    /// decoders. Empty until the first `InventoryReady` event lands.
    #[serde(default)]
    pub inventory: Inventory,
    /// Live snapshot of the reactor's current `Goal`. The reactor emits
    /// `ReactorGoalChanged` on every mutation; the fold here mirrors it
    /// for renderers (Bevy HUD) and resources (`goal://current` could
    /// soon read this in addition to `goal_store`).
    #[serde(default)]
    pub current_goal: Option<ReactorGoalSnapshot>,
    /// Most recent supervisor reconnect, for display in the operator HUD.
    #[serde(default)]
    pub last_reconnect: Option<ReconnectInfo>,
    /// Bounded ring of recent LLM decisions (notifications fired + tool
    /// dispatches), capped at `RECENT_DECISIONS_CAP`. Drives the
    /// LLM-decision badge in the operator dashboard.
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    pub recent_decisions: VecDeque<LlmDecision>,
    #[serde(default = "default_fps")]
    pub target_fps: u32,
    /// Bounded ring of recent LLM decisions...
    /// Bounded ring of recent name-extraction misses — diagnostic surface
    /// for "?" entities. Each entry captures the raw packet body so the
    /// MCP-side AI can audit the SendFlg byte and name-slot offset
    /// without rebuilding ffxi-client with extra logging. Capped at
    /// `NAME_MISSES_CAP`. Exposed as the `debug://name_misses` MCP
    /// resource.
    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    pub name_misses: VecDeque<NameExtractionMiss>,
    /// Active NPC event dialog metadata. `Some(...)` from `EventDialog`
    /// arrival until `EventEnded` clears it. Mirrored to the wire in
    /// `wire_translate::dialog_to_wire`. The fields are the wire ground
    /// truth — no DAT text — so the operator sees event_id, NPC reference,
    /// mode, and runtime parameters but not the canned English dialog
    /// (that lives in client-side DAT files we don't ship).
    #[serde(default)]
    pub dialog: Option<DialogState>,
    /// Active NPC shop window. `Some(...)` while a shop is open; cleared
    /// on `EventEnded`. Driven by 0x03C `SHOP_LIST` (item rows) and
    /// 0x03E `SHOP_OPEN` (the "show window" signal).
    #[serde(default)]
    pub shop: Option<ShopState>,
    /// Active status-effect icon ids on the operator. Decoded from 0x063
    /// type=0x09 STATUS_ICONS. Placeholder slots (`0x00FF`) are dropped
    /// before insertion so the list represents only real effects.
    #[serde(default)]
    pub status_icons: Vec<u16>,
    /// Most recent `WeatherNumber` from 0x057 WEATHER. `None` until the
    /// first weather packet for the current zone arrives, and cleared
    /// on zone change since the new zone always re-sends weather.
    /// Stored as the raw LSB index (per `vendor/server/src/map/enums/weather.h`)
    /// to keep `state.rs` decoupled from the optional `ffxi-viewer-wire`
    /// crate — same rationale as `DialogState` / `ShopState` mirroring.
    /// Mapped to `ffxi_viewer_wire::Weather` via `Weather::from_lsb` at
    /// snapshot time in `wire_translate::state_to_snapshot` (pending the
    /// wire-side `SceneSnapshot.weather` field landing — currently absent
    /// from `ffxi-viewer-wire`).
    #[serde(default)]
    pub current_weather: Option<u16>,
}

/// Mirror of `ffxi_viewer_wire::DialogState` defined locally so `state.rs`
/// stays decoupled from the optional `ffxi-viewer-wire` crate (only
/// pulled in via the `native-window` / `relay` features).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DialogState {
    pub event_id: u32,
    pub npc_id: u32,
    /// See `ffxi_viewer_wire::DialogState::npc_name`.
    #[serde(default)]
    pub npc_name: Option<String>,
    pub act_index: u16,
    pub event_num: u16,
    pub event_para: u16,
    pub mode: u16,
    pub event_num2: u16,
    pub event_para2: u16,
    pub strings: Vec<String>,
    pub nums: Vec<i32>,
}

/// Mirror of `ffxi_viewer_wire::ShopState`. Same decoupling rationale as
/// `DialogState`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShopState {
    pub offset_index: u16,
    pub items: Vec<ShopItem>,
    pub opened: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShopItem {
    pub price: u32,
    pub item_no: u16,
    pub shop_index: u8,
    pub skill: u16,
    pub guild_info: u16,
}

/// Inventory mirror, populated by Stage-9 decoders. Container ids match
/// `Phoenix/src/map/packets/s2c/0x01c_item_max.h`'s `Container` enum
/// (0=Inventory, 1=Safe, 2=Storage, …, 13=Wardrobe1, …). Storing as a
/// HashMap keeps the layer flexible against new container slots in
/// upstream LSB without breaking serialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Inventory {
    pub containers: HashMap<u8, ContainerInfo>,
    /// Set when 0x01D `ITEM_SAME` arrives with `State::AllLoaded`.
    /// Indicates the initial zone-in inventory flood has finished and the
    /// agent can rely on capacity / slot counts being authoritative.
    pub all_loaded: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub capacity: u8,
    pub slots: Vec<ItemSlot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemSlot {
    pub index: u8,
    pub item_no: u16,
    pub quantity: u32,
    pub locked: bool,
    pub price: u32,
}

/// Specifics of an inventory mutation. The umbrella `InventoryUpdated`
/// AgentEvent carries one of these so `apply_event` can fold the change
/// into `SessionState.inventory` without the session loop touching
/// state directly. Mirrors the union of the four "inventory-mutating"
/// s2c packets: `0x01C ITEM_MAX` (capacity table), `0x01E ITEM_NUM`
/// (quantity-only), `0x01F ITEM_LIST` (full slot), `0x020 ITEM_ATTR`
/// (full slot + extdata, which we discard in state).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InventoryUpdate {
    /// 0x01C ITEM_MAX — full capacity table for all 18 CONTAINER_IDs.
    /// The fold writes each non-zero capacity into the corresponding
    /// `ContainerInfo`; zero-capacity entries leave the container
    /// unmodified (the table is sparse — only meaningful entries are
    /// populated by the server).
    Capacities { capacities: Vec<u16> },
    /// 0x01F ITEM_LIST or 0x020 ITEM_ATTR — full slot definition. The
    /// fold replaces (or inserts) the slot at `slot.index`. A
    /// `quantity == 0` removes the slot — that's how upstream LSB
    /// clears a slot when the player drops the last stack.
    SlotChanged { slot: ItemSlot },
    /// 0x01E ITEM_NUM — quantity-only update for an existing slot. If
    /// the slot doesn't exist yet (race with ITEM_LIST during zone-in),
    /// the update is dropped — ITEM_LIST will arrive with the full row.
    /// `quantity == 0` removes the slot.
    QuantityChanged {
        index: u8,
        quantity: u32,
        locked: bool,
    },
}

/// Serializable mirror of `crate::reactor::Goal`. Lives here (not in
/// reactor.rs) so the state crate can be consumed without pulling in
/// reactor internals — the renderers care about *what* the goal is, not
/// how the reactor implements it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReactorGoalSnapshot {
    Idle,
    Following {
        target_id: u32,
        distance: f32,
    },
    Engaged {
        target_id: u32,
        attack_issued: bool,
    },
    /// Pathing toward `(x, y, z)`. Stage 10b added multi-waypoint
    /// pathing via `ffxi-nav`; `waypoints_remaining` is the count of
    /// waypoints still ahead of the agent (including the destination
    /// itself), so a fresh straight-line path reads as 1 and a navmesh
    /// path with three corners reads as 3 and counts down. Renderers
    /// that don't care can keep showing just the final destination.
    Pathing {
        x: f32,
        y: f32,
        z: f32,
        #[serde(default = "one_u32")]
        waypoints_remaining: u32,
    },
    /// Stage-9 reactor goal: monitor inventory and zone to mog house when
    /// any non-mog container reaches `threshold` slots filled.
    Banking {
        threshold: u8,
        mog_house_zoneline: u32,
    },
}

impl Default for ReactorGoalSnapshot {
    fn default() -> Self {
        ReactorGoalSnapshot::Idle
    }
}

fn one_u32() -> u32 {
    1
}

/// Last supervisor reconnect, surfaced to the operator HUD.
/// `monotonic_at_unix_ms` is wall-clock — the dashboard compares against
/// `SystemTime::now()` to render "1.2s ago". Wall-clock (not monotonic)
/// because `SessionState` round-trips through serde and crosses process
/// boundaries; pairing two `Instant`s across processes is meaningless.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectInfo {
    pub downtime_ms: u64,
    pub at_unix_ms: u64,
}

/// One LLM-decision data point: either a notification we fired toward
/// the harness, or a tool the harness dispatched. Pairing across the
/// two surfaces the round-trip "thinking time" the operator sees.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmDecision {
    pub kind: LlmDecisionKind,
    /// Microseconds elapsed for the in-process side (notification send,
    /// tool dispatch). LLM round-trip across `NotificationFired →
    /// ToolDispatched` is computed at render time by pairing entries.
    pub latency_us: u64,
    /// Process-monotonic timestamp in milliseconds since process start.
    /// Used by the dashboard's "pulse" decay; not meaningful across
    /// processes.
    pub at_monotonic_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmDecisionKind {
    /// MCP fired a `notifications/resources/updated` toward the harness.
    NotificationFired { uri: String },
    /// Harness invoked a tool; `tool` is the cmd_kind_label.
    ToolDispatched { tool: String },
}

/// Cap on retained decisions. The HUD sparkline shows the most recent
/// 32; 64 leaves headroom for histograms without unbounded growth.
const RECENT_DECISIONS_CAP: usize = 64;

/// Classification of *why* `PosHead::try_extract_name` returned `None`
/// for a CHAR_PC / CHAR_NPC packet. Reported alongside the raw body so
/// downstream auditors can tell "server didn't send the name this tick"
/// (expected) from "we mishandled the name slot" (bug).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameMissKind {
    /// `body[6] & 0x08 == 0` — LSB did not include the name in this
    /// packet. Expected for position-only follow-ups; means the client
    /// must fall back on a prior spawn packet (or the LOGIN seed) for
    /// the name. If you see this on *every* packet for a given entity
    /// across a minute or so, the spawn packet was missed.
    NameBitClear,
    /// Name bit is set but the extractor returned `None` — either the
    /// body is shorter than the name offset, or the slot failed ASCII
    /// validation. This is the signature of a remaining offset bug.
    NameBitSetExtractionFailed,
}

/// One captured name-extraction miss, with enough context to audit the
/// packet byte-by-byte. `body_hex` is truncated to the first 96 bytes
/// — name slots live well within that window (CHAR_PC slot starts at
/// 0x5A; CHAR_NPC at 0x30).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameExtractionMiss {
    pub opcode: u16,
    pub unique_no: u32,
    pub act_index: u16,
    /// `body[6]` — SendFlg byte for CHAR_PC, updatemask byte for CHAR_NPC.
    pub send_flag: u8,
    pub body_len: usize,
    /// Lowercase hex of the first `min(body_len, 96)` bytes of the body.
    pub body_hex: String,
    pub miss_kind: NameMissKind,
    /// `SystemTime::now()` epoch-ms when the miss was captured. Lets
    /// the auditor pair the miss against the entity's last `EntityUpserted`.
    pub at_unix_ms: u64,
}

/// Cap on retained name-extraction misses. 64 covers ~a minute of
/// distinct misses at the rate-limited emit cadence.
const NAME_MISSES_CAP: usize = 64;

/// One row in the agent's view of the party. Populated from
/// `0x0DD GROUP_LIST` (other members; provides name + leader flags) and
/// `0x0DF GROUP_ATTR` (self + Trusts; HP/MP/TP refreshes only). Apply
/// rules: same-id updates merge, with `name`/`is_party_leader` preserved
/// across attr-only refreshes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartyMember {
    pub id: u32,
    pub act_index: u16,
    pub name: Option<String>,
    pub hp: u32,
    pub mp: u32,
    pub tp: u32,
    pub hp_pct: u8,
    pub mp_pct: u8,
    pub zone_no: u16,
    pub main_job: u8,
    pub main_job_lv: u8,
    pub sub_job: u8,
    pub sub_job_lv: u8,
    pub is_party_leader: bool,
    pub is_alliance_leader: bool,
    /// `MoghouseFlg` from the wire (0x0DF body[21] / 0x0DD body[23]).
    /// Non-zero when the server treats this member as inside a Mog House
    /// (`PChar->m_moghouseID != 0`). For self this is the *only* wire signal
    /// of mog-house occupancy — `zone_id` stays equal to the surrounding
    /// city. See `SessionState::self_in_mog_house`.
    #[serde(default)]
    pub in_mog_house: bool,
}

/// Cap on retained chat history. The TUI only ever shows the last N visible
/// lines; older entries are dropped to keep allocations bounded under long
/// sessions. 256 is generous for ~10 minutes of social chat.
const CHAT_HISTORY_CAP: usize = 256;

impl SessionState {
    /// Player's position derived from the self entity in the entity list
    /// (the one whose `id == self.char_id`). Returns `None` if `char_id`
    /// isn't known yet, or if no matching entity has been upserted —
    /// callers handling that case decide whether to substitute
    /// `Position::default()` or wait for the first `EntityUpserted` for
    /// self.
    ///
    /// This is the *only* source of self position. `WireSnapshot.self_pos`
    /// is populated from it. The previous duplicate `state.self_pos`
    /// field has been removed (Stage 5 of the
    /// `collapse-self-position-to-single-source-of-truth` refactor).
    /// `true` when the most recent `0x0DF GROUP_ATTR` for self (the row whose
    /// `id == char_id`) had `MoghouseFlg` set. Use this — not `zone_id` — to
    /// decide whether we're inside a Mog House: LSB keeps the zone id equal
    /// to the surrounding city while in a mog room. Returns `false` if we
    /// haven't seen a self-party row yet (during a reconnect the wire flag
    /// arrives a tick or two after LOGIN, so callers should treat the
    /// initial `false` as "unknown / probably normal").
    pub fn self_in_mog_house(&self) -> bool {
        let Some(char_id) = self.char_id else {
            return false;
        };
        self.party
            .iter()
            .find(|m| m.id == char_id)
            .map(|m| m.in_mog_house)
            .unwrap_or(false)
    }

    pub fn self_position(&self) -> Option<Position> {
        let char_id = self.char_id?;
        self.entities
            .iter()
            .find(|e| e.id == char_id)
            .map(|e| Position {
                pos: e.pos,
                heading: e.heading,
                speed: e.speed,
                speed_base: e.speed_base,
            })
    }

    /// Pure fold: apply an `AgentEvent` to derive the new state. Kept free of
    /// I/O so it's trivially testable and so a watch-based renderer never
    /// blocks the event-broadcast task.
    pub fn apply_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Connected {
                account_id,
                char_id,
                character,
                zone_id,
            } => {
                self.account_id = Some(*account_id);
                self.char_id = Some(*char_id);
                self.character = Some(character.clone());
                self.zone_id = Some(*zone_id);
            }
            AgentEvent::StageChanged { stage } => {
                self.stage = *stage;
                self.diagnostics.stage = Some(*stage);
            }
            AgentEvent::ZoneChanged { to, .. } => {
                self.zone_id = Some(*to);
                // Entities from the old zone are stale on zone change; the
                // new zone's flood will repopulate. Same goes for the
                // party roster: HP/MP percent values get a fresh
                // `party-attr` packet on the other side, and stale zero
                // HP would (a) misrender self_hud bars and (b) trigger
                // the death-prompt HUD spuriously when the player dies
                // → Returns-to-Home (Phoenix `HomePoint` restores HP,
                // but the new zone's party-attr arrives after the LOGIN
                // packet, so `is_dead` would briefly stay `true`).
                self.entities.clear();
                self.party.clear();
                // Weather is per-zone; the new zone will send a fresh
                // 0x057. Clearing here means consumers see `None` during
                // the brief zoning window rather than stale conditions
                // from the previous zone.
                self.current_weather = None;
            }
            AgentEvent::PositionChanged { pos } => {
                // Single source of truth: the self entity in the entity list.
                // Find the entry whose `id == self.char_id` and update its
                // position fields in place. If it doesn't exist yet (we
                // received `PositionChanged` before LOGIN/CHAR_PC seeded
                // the entity), no-op — the next CHAR_PC or LOGIN-seed
                // event will land with the real server-authoritative
                // value anyway.
                if let Some(char_id) = self.char_id {
                    if let Some(ent) = self.entities.iter_mut().find(|e| e.id == char_id) {
                        ent.pos = pos.pos;
                        ent.heading = pos.heading;
                        ent.speed = pos.speed;
                        ent.speed_base = pos.speed_base;
                    }
                }
            }
            AgentEvent::EntityUpserted { entity } => {
                if let Some(existing) = self.entities.iter_mut().find(|e| e.id == entity.id) {
                    // CHAR_NPC (0x0E) only includes the name on packets where
                    // the server set UPDATE_NAME — typically ENTITY_SPAWN and
                    // renamed-mob ticks (see vendor/server/.../entity_update.cpp).
                    // Position-only follow-ups arrive with body length < 64,
                    // making try_extract_name return None. Preserve the prior
                    // name on those so the target panel doesn't drop to "?".
                    let preserved_name = entity.name.clone().or_else(|| existing.name.clone());
                    let merged_kind = merge_kind(existing.kind, entity.kind);
                    // Same field-gating logic as `name`: the producer
                    // (session.rs CHAR_PC/CHAR_NPC handler) only fills
                    // `hp_pct` when the server set UPDATE_HP (0x04) in the
                    // updatemask. Position-only ticks arrive with
                    // `hp_pct: None`; preserve the prior value so the target
                    // panel doesn't oscillate against an uninitialized zero
                    // byte from the LSB packet buffer.
                    let preserved_hp_pct = entity.hp_pct.or(existing.hp_pct);
                    *existing = Entity {
                        name: preserved_name,
                        kind: merged_kind,
                        hp_pct: preserved_hp_pct,
                        ..entity.clone()
                    };
                } else {
                    self.entities.push(entity.clone());
                }
            }
            AgentEvent::EntityRemoved { id } => {
                self.entities.retain(|e| e.id != *id);
            }
            AgentEvent::NameExtractionMiss { miss } => {
                self.name_misses.push_back(miss.clone());
                while self.name_misses.len() > NAME_MISSES_CAP {
                    self.name_misses.pop_front();
                }
            }
            AgentEvent::EntityPatched {
                id,
                act_index,
                name,
                kind,
                hp_pct,
            } => {
                // Resolve target entity by id (preferred) or act_index. Drop
                // the patch if neither matches a known entity — pets and
                // trusts get a CHAR_NPC stream that creates the entry; this
                // patch is enrichment, not creation.
                let existing = self.entities.iter_mut().find(|e| {
                    id.is_some_and(|target| e.id == target)
                        || act_index.is_some_and(|target| e.act_index == target)
                });
                if let Some(existing) = existing {
                    if let Some(n) = name {
                        existing.name = Some(n.clone());
                    }
                    if let Some(k) = kind {
                        existing.kind = merge_kind(existing.kind, *k);
                    }
                    if let Some(hp) = hp_pct {
                        existing.hp_pct = Some(*hp);
                    }
                }
            }
            AgentEvent::ChatLine { line } => {
                self.chat.push(line.clone());
                if self.chat.len() > CHAT_HISTORY_CAP {
                    let drop = self.chat.len() - CHAT_HISTORY_CAP;
                    self.chat.drain(0..drop);
                }
            }
            AgentEvent::Diagnostics { diagnostics } => {
                self.diagnostics = diagnostics.clone();
            }
            AgentEvent::SetFps { max } => {
                self.target_fps = *max;
            }
            AgentEvent::Disconnected { .. } => {
                self.stage = Stage::Disconnected;
                self.diagnostics.stage = Some(Stage::Disconnected);
            }
            // Surface errors as system chat so the user sees them in the TUI
            // without needing a separate pane. The stage doesn't change —
            // many errors are recoverable.
            AgentEvent::Error { message } => {
                self.chat.push(ChatLine {
                    channel: ChatChannel::System,
                    sender: "<error>".into(),
                    text: message.clone(),
                    server_ts: 0,
                });
                if self.chat.len() > CHAT_HISTORY_CAP {
                    let drop = self.chat.len() - CHAT_HISTORY_CAP;
                    self.chat.drain(0..drop);
                }
            }
            AgentEvent::PartyMemberUpdated { member } => {
                if let Some(existing) = self.party.iter_mut().find(|m| m.id == member.id) {
                    // Preserve name / leader flags across 0x0DF attr-only updates
                    // (which don't carry them). The new packet always wins for
                    // HP/MP/TP/job/zone — those it always has.
                    let preserved_name = if member.name.is_some() {
                        member.name.clone()
                    } else {
                        existing.name.clone()
                    };
                    let preserved_leader = if !member.name.is_some() {
                        existing.is_party_leader
                    } else {
                        member.is_party_leader
                    };
                    let preserved_alliance = if !member.name.is_some() {
                        existing.is_alliance_leader
                    } else {
                        member.is_alliance_leader
                    };
                    *existing = PartyMember {
                        name: preserved_name,
                        is_party_leader: preserved_leader,
                        is_alliance_leader: preserved_alliance,
                        ..member.clone()
                    };
                } else {
                    self.party.push(member.clone());
                }
            }
            AgentEvent::Reconnected { downtime_ms } => {
                let at_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                self.last_reconnect = Some(ReconnectInfo {
                    downtime_ms: *downtime_ms,
                    at_unix_ms,
                });
            }
            AgentEvent::ReactorGoalChanged { goal } => {
                self.current_goal = Some(goal.clone());
            }
            AgentEvent::LlmDecision { decision } => {
                self.recent_decisions.push_back(decision.clone());
                while self.recent_decisions.len() > RECENT_DECISIONS_CAP {
                    self.recent_decisions.pop_front();
                }
            }
            AgentEvent::InventoryReady => {
                self.inventory.all_loaded = true;
            }
            // High-signal events the LLM wakes for. They don't mutate
            // SessionState — the data is already there (HP via entity
            // updates, chat via ChatLine). They're notifications, not state.
            AgentEvent::LowHp { .. }
            | AgentEvent::PartyMemberLowHp { .. }
            | AgentEvent::EngagedBy { .. }
            | AgentEvent::TellReceived { .. }
            | AgentEvent::SceneSummary { .. }
            | AgentEvent::HumanInControl { .. }
            | AgentEvent::HumanReleased => {}
            AgentEvent::InventoryUpdated { container, update } => {
                let entry = self.inventory.containers.entry(*container).or_default();
                match update {
                    InventoryUpdate::Capacities { capacities } => {
                        // Capacities are container-indexed; the
                        // umbrella event uses container=0 as a
                        // placeholder. Iterate and set each non-zero.
                        for (id, cap) in capacities.iter().enumerate() {
                            if *cap == 0 {
                                continue;
                            }
                            self.inventory
                                .containers
                                .entry(id as u8)
                                .or_default()
                                .capacity = (*cap).min(u8::MAX as u16) as u8;
                        }
                    }
                    InventoryUpdate::SlotChanged { slot } => {
                        if slot.quantity == 0 {
                            entry.slots.retain(|s| s.index != slot.index);
                        } else if let Some(existing) =
                            entry.slots.iter_mut().find(|s| s.index == slot.index)
                        {
                            *existing = slot.clone();
                        } else {
                            entry.slots.push(slot.clone());
                        }
                    }
                    InventoryUpdate::QuantityChanged {
                        index,
                        quantity,
                        locked,
                    } => {
                        if *quantity == 0 {
                            entry.slots.retain(|s| s.index != *index);
                        } else if let Some(existing) =
                            entry.slots.iter_mut().find(|s| s.index == *index)
                        {
                            existing.quantity = *quantity;
                            existing.locked = *locked;
                        }
                        // Race with ITEM_LIST: drop silently — the
                        // full-slot packet will arrive separately.
                    }
                }
            }
            // EventStart is the lean signal (event_id only); EventDialog
            // is its richer companion that fills in the dialog HUD state.
            // EventEnded clears the dialog. KeyRotated is internal-only.
            AgentEvent::EventStart { .. } | AgentEvent::KeyRotated { .. } => {}
            AgentEvent::EventDialog { dialog } => {
                self.dialog = Some(dialog.clone());
            }
            AgentEvent::ShopUpdated { shop } => {
                self.shop = Some(shop.clone());
            }
            AgentEvent::StatusIconsUpdated { icons } => {
                self.status_icons = icons.clone();
            }
            AgentEvent::WeatherUpdated { weather_number } => {
                self.current_weather = Some(*weather_number);
            }
            AgentEvent::EventEnded => {
                self.dialog = None;
                // Shops live inside an event in vanilla; clearing on
                // EventEnded matches the user-facing "talk → buy → walk
                // away" lifecycle without needing a separate shop-close
                // packet (none exists in the wire protocol — the server
                // just stops responding to 0x083s after the event ends).
                self.shop = None;
            }
        }
    }
}

impl Default for Stage {
    fn default() -> Self {
        Stage::Idle
    }
}

/// Events emitted by the Session actor. The JSON sidechannel writes these
/// one-per-line to stdout; the TUI consumes them via a `tokio::sync::broadcast`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Connected {
        account_id: u32,
        char_id: u32,
        character: String,
        zone_id: u16,
    },
    StageChanged {
        stage: Stage,
    },
    ZoneChanged {
        from: Option<u16>,
        to: u16,
    },
    PositionChanged {
        pos: Position,
    },
    EntityUpserted {
        entity: Entity,
    },
    EntityRemoved {
        id: u32,
    },
    /// Diagnostic event fired when `PosHead::try_extract_name` returns
    /// `None` for a CHAR_PC or CHAR_NPC packet. Captures the raw body
    /// so the MCP-side `debug://name_misses` resource can surface the
    /// bytes to an auditor without re-running the client with extra
    /// logging. Rate-limited at the emitter (one per `(id, kind)` per
    /// 30s) so it never floods the attach socket.
    NameExtractionMiss {
        miss: NameExtractionMiss,
    },
    /// Partial update for an existing entity — used by name-only packets
    /// (`0x067 CEntitySetNamePacket`, trust/fellow/pankration names) and
    /// pet enrichment (`0x068 CPetSyncPacket`). Either `id` or `act_index`
    /// must be set; the state handler resolves the entity, then applies
    /// only the `Some(_)` fields. If the entity is unknown (no prior
    /// CHAR_NPC), the patch is dropped — the next CHAR_NPC will spawn
    /// the entity and a subsequent patch can re-enrich it.
    EntityPatched {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        act_index: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<EntityKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hp_pct: Option<u8>,
    },
    ChatLine {
        line: ChatLine,
    },
    EventStart {
        event_id: u32,
    },
    /// Richer companion to `EventStart` — carries the full decoded
    /// `DialogState` so the viewer can render an NPC dialog HUD. Always
    /// emitted alongside `EventStart` (same packet); kept as a separate
    /// variant so the existing JSON contract for agents that only care
    /// about the `event_id` remains stable.
    EventDialog {
        dialog: DialogState,
    },
    /// 0x03C `SHOP_LIST` — full or partial item list for an NPC shop.
    /// The fold replaces `SessionState.shop`; partial-list reassembly
    /// (for shops with >19 items) lands when we see one in the wild.
    ShopUpdated {
        shop: ShopState,
    },
    /// Status-effect icon list refreshed (0x063 type=0x09). Carries the
    /// non-placeholder icon ids in display order; consumers should
    /// replace, not merge, since the server sends the full list every
    /// time an effect lands or expires.
    StatusIconsUpdated {
        icons: Vec<u16>,
    },
    /// 0x057 WEATHER — current zone weather. Carries the raw LSB
    /// `WeatherNumber`; consumers should map via
    /// `ffxi_viewer_wire::Weather::from_lsb`. The server re-sends one of
    /// these on every zone-in plus whenever weather changes mid-zone.
    WeatherUpdated {
        weather_number: u16,
    },
    EventEnded,
    KeyRotated {
        previous_status: BlowfishStatus,
    },
    Disconnected {
        reason: String,
    },
    Error {
        message: String,
    },
    Diagnostics {
        diagnostics: Diagnostics,
    },
    /// 0x0DD GROUP_LIST (other) or 0x0DF GROUP_ATTR (self + Trust). The
    /// merge rules in `apply_event` preserve `name` / `is_party_leader`
    /// across attr-only refreshes.
    PartyMemberUpdated {
        member: PartyMember,
    },
    /// Self HP crossed below a low-HP threshold (default 25%, configurable
    /// in the reactor). Wakes the LLM strategically without burning
    /// tokens on every HP tick.
    LowHp {
        pct: u8,
    },
    /// A party member's HP crossed below the low-HP threshold. Drives
    /// healer co-play.
    PartyMemberLowHp {
        id: u32,
        pct: u8,
    },
    /// A mob took the player as its battle target — i.e. aggro. The agent
    /// may want to react (engage, flee, kite).
    EngagedBy {
        entity_id: u32,
    },
    /// Set the target frame rate.
    SetFps {
        max: u32,
    },
    /// Received a /tell from another player. The most user-directed
    /// channel; nearly always worth waking the LLM for.
    TellReceived {
        from: String,
        text: String,
    },
    /// Supervisor restored the session after a disconnect. The agent
    /// should re-orient to the (possibly changed) zone state.
    Reconnected {
        downtime_ms: u64,
    },
    /// Pre-rendered scene summary, emitted in response to `Snapshot`.
    /// Distinct from `Diagnostics` (operational) — this is the agent's
    /// view of "what's happening right now" in compact prose.
    SceneSummary {
        text: String,
    },
    /// One container's inventory contents changed. The `update`
    /// payload carries the specifics (capacity table / full-slot
    /// row / quantity-only delta); `apply_event` folds it into
    /// `SessionState.inventory.containers[container]`. The LLM wakes
    /// only for `InventoryReady` (initial flood done) — per-slot churn
    /// is tactical detail and the MCP notifier filters accordingly.
    InventoryUpdated {
        container: u8,
        update: InventoryUpdate,
    },
    /// Initial zone-in inventory flood is complete (0x01D `ITEM_SAME`
    /// with `State::AllLoaded`). After this point the agent can rely
    /// on container capacities and slot counts being authoritative for
    /// `bank_when_full` checks.
    InventoryReady,
    /// Reactor goal transitioned. Mirrored into
    /// `SessionState.current_goal` so renderers can show the active
    /// intent. Fires on every `handle_command` mutation and on the
    /// `Pathing → Idle` self-clearing tick.
    ReactorGoalChanged {
        goal: ReactorGoalSnapshot,
    },
    /// One LLM-decision data point. Emitted from the in-process MCP
    /// server (combined-binary mode) — the headless `ffxi-mcp` does not
    /// emit these because it has no broadcast peer.
    LlmDecision {
        decision: LlmDecision,
    },
    /// The native operator has paused agent control via `/agent pause`.
    /// Subsequent agent-originated commands (from `agent_codec` /
    /// `agent_socket`) will be silently dropped until the matching
    /// [`HumanReleased`](Self::HumanReleased) fires. GUI slash commands
    /// continue to flow through unchanged — the operator is taking the
    /// stick. Well-behaved agents see this event and stop initiating
    /// actions; the wire-level gate is the agent_codec drop, but the
    /// event lets the harness log the takeover and the LLM stand down
    /// cooperatively rather than having its tool calls disappear into
    /// the void.
    HumanInControl {
        /// Free-form context, e.g. why the operator took over. May be
        /// empty.
        reason: String,
    },
    /// `/agent resume` — the operator released control back to the
    /// agent. Pair with [`HumanInControl`](Self::HumanInControl).
    HumanReleased,
}

/// Strictly enumerated commands an agent can issue. **Do not** add a generic
/// `SendPacket` escape hatch — Claude Code knows about FFXI from training and
/// will hallucinate opcodes; the lid stays on by design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum AgentCommand {
    /// Set the next position the keepalive will report.
    Move { x: f32, y: f32, z: f32, heading: u8 },
    /// Stop sending position updates (revert keepalive to last-known position).
    StopMove,
    /// Request a zone change at a known zoneline. Server still has to honor it.
    RequestZoneChange { line_id: u32 },
    /// `GP_CLI_COMMAND_MAPRECT` (0x05E) with `RectID="zmrq"` — the universal
    /// "leave my Mog House" packet. LSB's `m_moghouseID != 0` state is only
    /// cleared by this packet (see `0x05e_maprect.cpp:134-164`); neither
    /// `/logout` nor crossing a normal zoneline does the trick, and the
    /// zone id stays equal to the surrounding city the whole time. `kind`
    /// chooses which exit the server emulates: `Home` is "step back to the
    /// area I entered from" (1F/regular Mog House exit), the per-city kinds
    /// (`Sandoria`, `Bastok`, …) use the "exit to alternate city zoneline"
    /// quest reward, and `Mog2F` / `Mog1F` flip between floors of the same
    /// Mog House without leaving. `MogGarden` zones out to the Mog Garden.
    MogHouseExit { kind: MogHouseExit },
    /// Auto-end any in-progress event/cutscene. Sends `EndPara=0` for the
    /// queued event id — appropriate for informational dialogs that need
    /// a single "advance" press.
    EndEvent,
    /// End the in-progress event with a specific `choice` (the server's
    /// `EndPara`). For multi-option NPC dialogs, `choice` selects which
    /// branch — typically 0..7 indexing into the option list. Targets one
    /// specific event — `event_id`/`act_index`/`event_num` come from the
    /// `AgentEvent::EventDialog` payload that opened the dialog so we
    /// don't accidentally close someone else's queued event.
    EndEventChoice {
        event_id: u32,
        act_index: u16,
        event_num: u16,
        choice: u32,
    },
    /// Drop the TCP connections (lobby + map) without going through the
    /// in-world `/logout` flow. Use this for "abandon the session right
    /// now" — process exit, crash recovery, keepalive timeout. Players
    /// who want a clean retail-style return to char-select should issue
    /// [`AgentCommand::ReqLogout`] instead, which respects the server's
    /// `EFFECT_LEAVEGAME` validator (no logout while in-event /
    /// abnormal-status / crafting).
    Disconnect,
    /// `GP_CLI_COMMAND_REQLOGOUT` (0x0E7) — request `/logout` (return to
    /// character select) or `/shutdown` (exit game) the in-world way.
    /// Server arms `EFFECT_LEAVEGAME`; for normal players the Lua
    /// handler runs `leaveGame()` after ~30s, but GMs and players
    /// inside a Mog House short-circuit to immediate disconnect (see
    /// `scripts/effects/leavegame.lua::onEffectGain`). The s2c 0x00B
    /// `LOGOUT` lands once `leaveGame()` fires. Toggling again while
    /// the effect is active cancels it (matching retail UI confirm
    /// behavior). Each [`ReqLogoutKind`] variant maps to one
    /// server-validated `(Mode, Kind)` pair — agents can't pair
    /// forbidden combinations.
    ReqLogout { kind: ReqLogoutKind },
    /// Echo back the current SessionState as a JSON event (debugging convenience).
    Snapshot,
    /// Send a chat-channel message. Server-side say messages beginning with
    /// `@` are dispatched as GM commands when the account has gmlevel ≥ 1.
    /// `kind` matches `GP_CLI_COMMAND_CHAT_STD::Kind` (0=say, 1=shout, 4=party, …).
    Chat { kind: u8, text: String },
    /// Send a `/tell` to another player. Uses `GP_CLI_COMMAND_CHAT_NAME`
    /// (0x0B6) — a separate opcode from `Chat`. `to` is the recipient's
    /// character name; the server resolves it cross-zone.
    Tell { to: String, text: String },
    /// `GP_CLI_COMMAND_ACTION` — universal "do thing to target" packet.
    /// `target_index` is the server-side `ActIndex` of the entity to target;
    /// `target_id` is its `UniqueNo`. `kind` is a strictly enumerated
    /// `ActionKind` carrying both the action type and any sub-parameters
    /// (spell id, weaponskill id, mount id, ground-target position, …).
    /// Strict enumeration is the lid: an agent cannot pair an arbitrary
    /// `action_id` with mismatched parameters — the type system says no.
    Action {
        target_id: u32,
        target_index: u16,
        kind: ActionKind,
    },
    /// Accept the home-point warp menu (sent automatically by the official
    /// FFXI client after death, when the player picks "Return to home
    /// point"). Wraps `ActionKind::HomepointMenu { status_id: 0 }` (Accept)
    /// but doesn't require the caller to know the player's own
    /// `(UniqueNo, ActIndex)` — the session loop fills those from the
    /// session-tracked self identity. Phoenix's handler ignores the wire
    /// `(UniqueNo, ActIndex)` for this `ActionID` (acts on `PChar`
    /// directly, see `0x01a_action.cpp::HomepointMenu`), so a stale
    /// `self_act_index` still yields the same outcome.
    ReturnToHomePoint,
    /// Set the target frame rate.
    SetFps { max: u32 },
    /// Reactor goal: keep stepping toward `target_id`, holding `distance`
    /// yalms once close. Works for follow-leader (party PC) and chase-mob.
    /// Handled by `crate::reactor`; if it reaches the session loop the
    /// reactor wasn't wired in front and the command is logged as an error.
    Follow { target_id: u32, distance: f32 },
    /// Reactor goal: face `target_id` and engage auto-attack. The reactor
    /// emits a single `Action::Attack` on transition, then keeps facing.
    Engage { target_id: u32 },
    /// Reactor goal: walk to `(x, y, z)` along a single straight segment.
    /// Multi-waypoint paths land in a future iteration.
    PathTo { x: f32, y: f32, z: f32 },
    /// Reactor: clear any active goal, return to `Idle`.
    Cancel,
    /// `GP_CLI_COMMAND_ITEM_USE` (0x037) — use a consumable / equipment
    /// item. **Not** an `ActionKind` variant: the wire opcode is `0x037`,
    /// not `0x01A`. `container` is the storage id (0=Inventory, 1=Safe,
    /// 8=Wardrobe, …) and `slot` is the property-item index inside that
    /// container; together they identify the item server-side. `item_no`
    /// is the FFXI item id (the LLM's bookkeeping hint — Stage 9's
    /// inventory mirror will let agents look this up themselves; until
    /// then it's passed inline so `use_item` doesn't soft-depend on
    /// Stage 9). The wire `ItemNum` field is **always sent as 0** —
    /// Phoenix's `0x037_item_use.cpp::validate` enforces
    /// `mustEqual(this->ItemNum, 0)`. `target_id` / `target_index`
    /// identify the recipient (self for potions / scrolls; another
    /// entity for ranged items like Soultrapper).
    UseItem {
        container: u8,
        slot: u8,
        item_no: u32,
        target_id: u32,
        target_index: u16,
    },
    /// Reactor goal: monitor inventory; when any non-mog container reaches
    /// `threshold` slots filled, request a zone change to `mog_house_zoneline`.
    /// Survives reconnects via `goal_store`. One-shot per banking cycle —
    /// once the zone change fires, the goal clears.
    BankWhenFull {
        threshold: u8,
        mog_house_zoneline: u32,
    },
    /// `GP_CLI_COMMAND_SHOP_BUY` (0x083) — purchase from an open NPC shop.
    /// `shop_index` is the row in the shop list (0..N-1, matching
    /// `ShopItem.shop_index`). `qty` is the number of items to buy; for
    /// most NPC shops the server caps this server-side based on the row.
    /// `shop_no` is the `ShopItemOffsetIndex` from the matching 0x03C
    /// header (echoed back so the server resolves which list).
    ShopBuy {
        shop_no: u16,
        shop_index: u8,
        qty: u32,
    },
    /// `GP_CLI_COMMAND_EQUIP_INSPECT` (0x0DD) — `/check` family. Asks the
    /// server to send back the target's inspection info via 0x0C9
    /// (`equip_inspect_general` / `equip_inspect_equipment`). Distinct
    /// opcode from `Action`/0x01A; the FFXI client uses 0x0DD for /check,
    /// /checkname, /checkparam. `kind` selects which.
    CheckTarget {
        target_id: u32,
        target_index: u16,
        kind: CheckKind,
    },
    /// `GP_CLI_COMMAND_CAMP` (0x0E8) — `/heal`. Toggles the resting
    /// (`EFFECT_HEALING`) state. Server validation rejects this packet
    /// when the player is engaged, in event, crafting, or under an
    /// abnormal status (see `vendor/server/src/map/packets/c2s/0x0e8_camp.cpp`).
    /// Cancelling heal *also* cancels an in-progress `EFFECT_LEAVEGAME`
    /// via the heal effect's Lua `onEffectLose` — so `/heal off` while
    /// /logout-armed effectively cancels the logout. The session loop
    /// also intercepts position-changing keepalives while healing and
    /// prepends a `Mode::Off` packet so movement implicitly cancels
    /// rest, matching retail-client behavior.
    Heal { mode: HealMode },
}

/// Sub-kind for [`AgentCommand::CheckTarget`]. Mirrors
/// `GP_CLI_COMMAND_EQUIP_INSPECT_KIND` in
/// `vendor/server/src/map/packets/c2s/0x0dd_equip_inspect.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// `/check` — full inspect (level estimate + visible equipment).
    Check,
    /// `/checkname` — short-form check (name + race + sex).
    CheckName,
    /// `/checkparam` — base parameter check.
    CheckParam,
}

impl CheckKind {
    /// Server `Kind` byte. Matches `GP_CLI_COMMAND_EQUIP_INSPECT_KIND`.
    pub fn as_u8(self) -> u8 {
        match self {
            CheckKind::Check => 0,
            CheckKind::CheckName => 1,
            CheckKind::CheckParam => 2,
        }
    }
}

/// Sub-kind for [`AgentCommand::Heal`]. Mirrors `GP_CLI_COMMAND_CAMP_MODE`
/// in `vendor/server/src/map/packets/c2s/0x0e8_camp.h`. `Toggle` is the
/// always-safe form — server picks the direction based on current
/// animation. `On`/`Off` are explicit and the server rejects mismatches
/// ("Requested healing when already healing" / "Requested stop healing
/// when not healing").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealMode {
    /// `/heal` (no arg) — toggle resting on/off; server decides direction.
    Toggle,
    /// `/heal on` — explicitly start resting. Rejected if already healing.
    On,
    /// `/heal off` — explicitly stop resting. Rejected if not healing.
    Off,
}

impl HealMode {
    /// Server `Mode` u32. Matches `GP_CLI_COMMAND_CAMP_MODE`.
    pub fn as_u32(self) -> u32 {
        match self {
            HealMode::Toggle => 0,
            HealMode::On => 1,
            HealMode::Off => 2,
        }
    }
}

/// Sub-kind for [`AgentCommand::ReqLogout`]. Each variant pins down one
/// server-validated `(Mode, Kind)` pair on the 0x0E7 wire packet —
/// `Mode` selects toggle / arm / cancel and `Kind` selects logout vs.
/// shutdown. Wire enums live in `ffxi_proto::map::reqlogout::{mode, kind}`.
///
/// `Off` is symmetric on the server — cancelling a LeaveGame works the
/// same way regardless of which kind originally armed it — but we keep
/// both `LogoutOff` and `ShutdownOff` so the variant carries which slash
/// command the user typed (useful for echo / event tracing without
/// having to also stash a tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReqLogoutKind {
    /// `/logout` — toggle a return-to-char-select timer (~30s for
    /// normal players; immediate for GMs / inside Mog House). (Mode=Toggle, Kind=Logout)
    LogoutToggle,
    /// `/logout on` — arm the logout timer; no-op if already running. (Mode=LogoutOn, Kind=Logout)
    LogoutOn,
    /// `/logout off` — cancel any in-progress LeaveGame. (Mode=Off, Kind=Logout)
    LogoutOff,
    /// `/shutdown` — toggle an exit-to-pol timer (~30s for normal
    /// players; immediate for GMs / inside Mog House). (Mode=Toggle, Kind=Shutdown)
    ShutdownToggle,
    /// `/shutdown on` — arm the shutdown timer; no-op if already running. (Mode=ShutdownOn, Kind=Shutdown)
    ShutdownOn,
    /// `/shutdown off` — cancel any in-progress LeaveGame. (Mode=Off, Kind=Shutdown)
    ShutdownOff,
}

impl ReqLogoutKind {
    /// Resolve to the wire `(Mode, Kind)` pair for the 0x0E7 body.
    pub fn wire_pair(self) -> (u16, u16) {
        use ffxi_proto::map::reqlogout::{kind, mode};
        match self {
            ReqLogoutKind::LogoutToggle => (mode::TOGGLE, kind::LOGOUT),
            ReqLogoutKind::LogoutOn => (mode::LOGOUT_ON, kind::LOGOUT),
            ReqLogoutKind::LogoutOff => (mode::OFF, kind::LOGOUT),
            ReqLogoutKind::ShutdownToggle => (mode::TOGGLE, kind::SHUTDOWN),
            ReqLogoutKind::ShutdownOn => (mode::SHUTDOWN_ON, kind::SHUTDOWN),
            ReqLogoutKind::ShutdownOff => (mode::OFF, kind::SHUTDOWN),
        }
    }
}

/// Sub-kind for [`AgentCommand::MogHouseExit`]. Each variant pins down one
/// server-validated `(MyRoomExitBit, MyRoomExitMode)` pair on the 0x05E
/// `MAPRECT` wire packet sent with `RectID="zmrq"` (the universal Mog House
/// exit tag, `Phoenix/src/map/packets/c2s/0x05e_maprect.cpp:72`). Variants
/// fall into three groups:
///
/// 1. **`Home`** — `(SandOria, AreaEnteredFrom)`. Equivalent to the player
///    walking out of their Mog House the way they came in. Always succeeds
///    when `inMogHouse()`; ignores `MyRoomExitBit`. **Preferred default**
///    because it never needs a quest flag.
/// 2. **`Sandoria` / `Bastok` / `Windurst` / `Jeuno` / `Whitegate` /
///    `Adoulin`** — "exit to a city in a region you have the Mog House
///    quest flag for" (the four `Option1..Option4` slots get encoded into
///    each region's specific zone via `0x05e_maprect.cpp:96-123`). The
///    caller passes `slot` to pick which sub-zone (e.g. for SandOria:
///    1=S.Sandy, 2=N.Sandy, 3=Port, 4=Chateau). Rejected if the player
///    hasn't unlocked the corresponding `mhflag` bit; you'll see a
///    `ShowWarning("Moghouse zoneline abuse")` on the server side.
/// 3. **`Mog1F` / `Mog2F` / `MogGarden`** — special exit-modes that don't
///    *leave* the Mog House so much as relocate inside it (or zone to the
///    Mog Garden). `Mog2F` requires `mhflag & 0x20` (2F unlocked); the
///    server warns and rejects otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MogHouseExit {
    /// Walk back out the door you came in through. The safe default.
    Home,
    /// Sandoria-region exit to one of S.Sandy/N.Sandy/Port/Chateau.
    /// `slot` ∈ {1,2,3,4}.
    Sandoria { slot: u8 },
    /// Bastok-region exit to one of Mines/Markets/Port/Metalworks.
    Bastok { slot: u8 },
    /// Windurst-region exit to one of Waters/Walls/Port/Woods.
    Windurst { slot: u8 },
    /// Jeuno-region exit to one of Ru'Lude/Upper/Lower/Port.
    Jeuno { slot: u8 },
    /// West-Aht-Urhgan exit (Al Zahbi / Whitegate).
    Whitegate { slot: u8 },
    /// Adoulin exit (`slot=1` West, `slot=2` East).
    Adoulin { slot: u8 },
    /// Move to Mog House 1F (no-op if already on 1F).
    Mog1F,
    /// Move to Mog House 2F. Requires the 2F unlock flag server-side.
    Mog2F,
    /// Exit to Mog Garden zone.
    MogGarden,
}

impl MogHouseExit {
    /// Resolve to the wire `(MyRoomExitBit, MyRoomExitMode)` pair for the
    /// 0x05E body. Bit values from
    /// `0x05e_maprect.h::GP_CLI_COMMAND_MAPRECT_MYROOMEXITBIT`; mode values
    /// from `MYROOMEXITMODE` (`AreaEnteredFrom=0`, `Option1..4=1..4`,
    /// `Mog2F=125`, `Mog1F=126`, `MogGarden=127`).
    pub fn wire_pair(self) -> (u8, u8) {
        match self {
            MogHouseExit::Home => (1, 0), // SandOria bit + AreaEnteredFrom; bit ignored
            MogHouseExit::Sandoria { slot } => (1, slot),
            MogHouseExit::Bastok { slot } => (2, slot),
            MogHouseExit::Windurst { slot } => (3, slot),
            MogHouseExit::Jeuno { slot } => (4, slot),
            MogHouseExit::Whitegate { slot } => (5, slot),
            MogHouseExit::Adoulin { slot } => (9, slot),
            MogHouseExit::Mog1F => (0, 126),
            MogHouseExit::Mog2F => (0, 125),
            MogHouseExit::MogGarden => (0, 127),
        }
    }
}

/// Tagged-union of every `0x01A` action the agent can perform. The variant
/// chosen determines both the wire `ActionID` and the layout of the 16-byte
/// `ActionBuf` payload — this is the typed alternative to letting the agent
/// invent (action_id, buf) pairs.
///
/// Mirrors `Phoenix/src/map/packets/c2s/0x01a_action.h`. Variants are
/// additive — when a new action type ships in LSB upstream, add a variant
/// here without breaking existing agents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionKind {
    /// 0x00 — Interact with NPC/Trust.
    Talk,
    /// 0x02 — Engage auto-attack on `target_id`.
    Attack,
    /// 0x03 — Cast magic. `spell_id` indexes into `Spells.dat`. `pos_*` are
    /// the ground-target position for AoE-target spells (Tractor, certain
    /// black/blue magic); zero for self/single-target casts.
    CastMagic {
        spell_id: u32,
        pos_x: f32,
        pos_y: f32,
        pos_z: f32,
    },
    /// 0x04 — Disengage auto-attack.
    AttackOff,
    /// 0x05 — Help flag (call for help on a mob).
    Help,
    /// 0x07 — Use a weaponskill.
    Weaponskill { skill_id: u32 },
    /// 0x09 — Use a job ability.
    JobAbility { ability_id: u32 },
    /// 0x0B — Respond to homepoint warp menu. 0=Accept, 1=MonstrosityCancel,
    /// 2=MonstrosityRetry.
    HomepointMenu { status_id: u32 },
    /// 0x0C — /assist (acquire your target's target).
    Assist,
    /// 0x0D — Respond to /raise menu.
    RaiseMenu { accept: bool },
    /// 0x0E — Fishing.
    Fish,
    /// 0x0F — Change target without engaging.
    ChangeTarget,
    /// 0x10 — Ranged attack.
    Shoot,
    /// 0x11 — Chocobo dig.
    ChocoboDig,
    /// 0x12 — Dismount.
    Dismount,
    /// 0x13 — Respond to /tractor menu.
    TractorMenu { accept: bool },
    /// 0x14 — Request character update from the server (rarely useful for
    /// agents; the server pushes updates unprompted).
    SendResRdy,
    /// 0x15 — Mining / Quarrying gather attempt.
    Quarry,
    /// 0x16 — Sprint (Run mode).
    Sprint,
    /// 0x17 — Scout.
    Scout,
    /// 0x18 — Toggle blockaid. 0=Disable, 1=Enable, 2=Toggle.
    Blockaid { status_id: u32 },
    /// 0x19 — Use a monster skill (Monstrosity).
    MonsterSkill { skill_id: u32 },
    /// 0x1A — Summon a mount (chocobo etc.) by `mount_id`.
    Mount { mount_id: u32 },
}

impl ActionKind {
    /// Wire `ActionID` for this action.
    pub fn action_id(&self) -> u16 {
        match self {
            ActionKind::Talk => 0x00,
            ActionKind::Attack => 0x02,
            ActionKind::CastMagic { .. } => 0x03,
            ActionKind::AttackOff => 0x04,
            ActionKind::Help => 0x05,
            ActionKind::Weaponskill { .. } => 0x07,
            ActionKind::JobAbility { .. } => 0x09,
            ActionKind::HomepointMenu { .. } => 0x0B,
            ActionKind::Assist => 0x0C,
            ActionKind::RaiseMenu { .. } => 0x0D,
            ActionKind::Fish => 0x0E,
            ActionKind::ChangeTarget => 0x0F,
            ActionKind::Shoot => 0x10,
            ActionKind::ChocoboDig => 0x11,
            ActionKind::Dismount => 0x12,
            ActionKind::TractorMenu { .. } => 0x13,
            ActionKind::SendResRdy => 0x14,
            ActionKind::Quarry => 0x15,
            ActionKind::Sprint => 0x16,
            ActionKind::Scout => 0x17,
            ActionKind::Blockaid { .. } => 0x18,
            ActionKind::MonsterSkill { .. } => 0x19,
            ActionKind::Mount { .. } => 0x1A,
        }
    }

    /// Fill the 16-byte `ActionBuf` slot per the union layout in
    /// `Phoenix/src/map/packets/c2s/0x01a_action.h`. Variants without a
    /// payload leave the buffer zero-filled, which the server tolerates.
    pub fn fill_action_buf(&self, buf: &mut [u8; 16]) {
        buf.fill(0);
        match self {
            ActionKind::CastMagic {
                spell_id,
                pos_x,
                pos_y,
                pos_z,
            } => {
                buf[0..4].copy_from_slice(&spell_id.to_le_bytes());
                buf[4..8].copy_from_slice(&pos_x.to_le_bytes());
                // Wire order is (PosX, PosZ, PosY) per ACTIONBUF_CASTMAGIC,
                // matching 0x015 POS — different from 0x05E MAPRECT's
                // (x, y, z). FFXI is inconsistent about this; mirror it.
                buf[8..12].copy_from_slice(&pos_z.to_le_bytes());
                buf[12..16].copy_from_slice(&pos_y.to_le_bytes());
            }
            ActionKind::Weaponskill { skill_id } | ActionKind::MonsterSkill { skill_id } => {
                buf[0..4].copy_from_slice(&skill_id.to_le_bytes());
            }
            ActionKind::JobAbility { ability_id } => {
                buf[0..4].copy_from_slice(&ability_id.to_le_bytes());
            }
            ActionKind::HomepointMenu { status_id } | ActionKind::Blockaid { status_id } => {
                buf[0..4].copy_from_slice(&status_id.to_le_bytes());
            }
            ActionKind::RaiseMenu { accept } | ActionKind::TractorMenu { accept } => {
                let id: u32 = if *accept { 0 } else { 1 };
                buf[0..4].copy_from_slice(&id.to_le_bytes());
            }
            ActionKind::Mount { mount_id } => {
                buf[0..4].copy_from_slice(&mount_id.to_le_bytes());
            }
            // Buf-less actions: Talk, Attack, AttackOff, Help, Assist, Fish,
            // ChangeTarget, Shoot, ChocoboDig, Dismount, SendResRdy, Quarry,
            // Sprint, Scout. Already zeroed.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `WeatherUpdated` folds onto `SessionState.current_weather`, and
    /// `ZoneChanged` clears it — verifying both halves of the per-zone
    /// lifecycle covered in the field's doc comment.
    #[test]
    fn weather_fold_sets_and_zone_change_clears() {
        let mut s = SessionState::default();
        assert_eq!(s.current_weather, None);
        s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 6 });
        assert_eq!(s.current_weather, Some(6));
        s.apply_event(&AgentEvent::ZoneChanged {
            from: Some(230),
            to: 231,
        });
        assert_eq!(s.current_weather, None);
    }

    #[test]
    fn agent_event_roundtrip() {
        let ev = AgentEvent::PositionChanged {
            pos: Position {
                pos: Vec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                heading: 64,
                ..Position::default()
            },
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::PositionChanged { pos } => {
                assert_eq!(pos.heading, 64);
                assert_eq!(pos.pos.y, 2.0);
            }
            _ => panic!("wrong variant: {back:?}"),
        }
    }

    #[test]
    fn agent_command_roundtrip() {
        let line = r#"{"cmd":"move","x":1.0,"y":2.0,"z":3.0,"heading":42}"#;
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        match cmd {
            AgentCommand::Move { x, y, z, heading } => {
                assert_eq!((x, y, z, heading), (1.0, 2.0, 3.0, 42));
            }
            _ => panic!("wrong variant: {cmd:?}"),
        }
    }

    #[test]
    fn action_kind_talk_decodes() {
        let line = r#"{"cmd":"action","target_id":42,"target_index":7,"kind":{"kind":"talk"}}"#;
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        match cmd {
            AgentCommand::Action {
                target_id,
                target_index,
                kind,
            } => {
                assert_eq!((target_id, target_index), (42, 7));
                assert!(matches!(kind, ActionKind::Talk));
                assert_eq!(kind.action_id(), 0x00);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn action_kind_castmagic_fills_buf() {
        let kind = ActionKind::CastMagic {
            spell_id: 0x101,
            pos_x: 1.5,
            pos_y: 0.0,
            pos_z: -2.5,
        };
        assert_eq!(kind.action_id(), 0x03);
        let mut buf = [0u8; 16];
        kind.fill_action_buf(&mut buf);
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 0x101);
        assert_eq!(f32::from_le_bytes(buf[4..8].try_into().unwrap()), 1.5);
        // Wire order is (PosX, PosZ, PosY) — matches ACTIONBUF_CASTMAGIC.
        assert_eq!(f32::from_le_bytes(buf[8..12].try_into().unwrap()), -2.5);
        assert_eq!(f32::from_le_bytes(buf[12..16].try_into().unwrap()), 0.0);
    }

    #[test]
    fn action_kind_weaponskill_fills_skill_id() {
        let kind = ActionKind::Weaponskill { skill_id: 0xCAFE };
        assert_eq!(kind.action_id(), 0x07);
        let mut buf = [0u8; 16];
        kind.fill_action_buf(&mut buf);
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 0xCAFE);
        // Trailing bytes stay zero.
        assert!(buf[4..].iter().all(|&b| b == 0));
    }

    #[test]
    fn party_member_upsert_preserves_name_across_attr_only_update() {
        let mut s = SessionState::default();
        let from_list = PartyMember {
            id: 42,
            act_index: 7,
            name: Some("Vanari".into()),
            hp: 2000,
            mp: 100,
            tp: 0,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 75,
            sub_job: 6,
            sub_job_lv: 37,
            is_party_leader: true,
            is_alliance_leader: false,
            in_mog_house: false,
        };
        s.apply_event(&AgentEvent::PartyMemberUpdated { member: from_list });
        assert_eq!(s.party.len(), 1);
        assert_eq!(s.party[0].name.as_deref(), Some("Vanari"));
        assert!(s.party[0].is_party_leader);

        // Subsequent 0x0DF GROUP_ATTR-shaped update: name None, leader false
        // (since attr-only). Must NOT clobber the preserved fields.
        let from_attr = PartyMember {
            id: 42,
            act_index: 7,
            name: None,
            hp: 1500, // took damage
            mp: 100,
            tp: 1234,
            hp_pct: 75,
            mp_pct: 100,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 75,
            sub_job: 6,
            sub_job_lv: 37,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: false,
        };
        s.apply_event(&AgentEvent::PartyMemberUpdated { member: from_attr });
        assert_eq!(s.party.len(), 1, "upsert by id");
        assert_eq!(s.party[0].name.as_deref(), Some("Vanari"), "name preserved");
        assert!(s.party[0].is_party_leader, "leader preserved");
        assert_eq!(s.party[0].hp, 1500, "HP overwritten");
        assert_eq!(s.party[0].hp_pct, 75);
    }

    #[test]
    fn action_kind_raise_menu_accept_zero_reject_one() {
        let mut buf = [0u8; 16];
        ActionKind::RaiseMenu { accept: true }.fill_action_buf(&mut buf);
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 0);
        ActionKind::RaiseMenu { accept: false }.fill_action_buf(&mut buf);
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 1);
    }

    #[test]
    fn apply_event_folds_in_documented_order() {
        let mut s = SessionState::default();
        assert_eq!(s.stage, Stage::Idle);

        s.apply_event(&AgentEvent::StageChanged {
            stage: Stage::Authenticating,
        });
        assert_eq!(s.stage, Stage::Authenticating);
        assert_eq!(s.diagnostics.stage, Some(Stage::Authenticating));

        s.apply_event(&AgentEvent::Connected {
            account_id: 42,
            char_id: 7,
            character: "Tester".into(),
            zone_id: 100,
        });
        assert_eq!(s.account_id, Some(42));
        assert_eq!(s.char_id, Some(7));
        assert_eq!(s.character.as_deref(), Some("Tester"));
        assert_eq!(s.zone_id, Some(100));

        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 999,
                act_index: 1,
                kind: EntityKind::Pc,
                name: Some("Other".into()),
                pos: Vec3 {
                    x: 1.0,
                    y: 0.0,
                    z: 2.0,
                },
                heading: 64,
                hp_pct: Some(80),
                bt_target_id: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
            },
        });
        assert_eq!(s.entities.len(), 1);

        // Re-upsert with same id should update, not duplicate.
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 999,
                act_index: 1,
                kind: EntityKind::Pc,
                name: Some("Other".into()),
                pos: Vec3 {
                    x: 5.0,
                    y: 0.0,
                    z: 6.0,
                },
                heading: 32,
                hp_pct: Some(50),
                bt_target_id: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
            },
        });
        assert_eq!(s.entities.len(), 1, "upsert must not duplicate by id");
        assert_eq!(s.entities[0].pos.x, 5.0, "upsert must overwrite");

        // ZoneChanged clears entities AND party (both are stale across
        // zone boundaries) and updates zone_id. The party clear is
        // load-bearing for the death-prompt HUD: post-Return-to-Home,
        // Phoenix restores HP server-side, but our snapshot's party
        // row carries the old zero-HP value until the new zone's
        // `party-attr` arrives. Without the clear, `is_dead` returns
        // true for ~100ms after zone-in, the prompt re-appears, and
        // an Enter press would re-dispatch HomepointMenu.
        s.party.push(PartyMember {
            id: 1,
            act_index: 1,
            name: None,
            hp: 0,
            mp: 0,
            tp: 0,
            hp_pct: 0,
            mp_pct: 100,
            zone_no: 100,
            main_job: 0,
            main_job_lv: 0,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: true,
            is_alliance_leader: false,
            in_mog_house: false,
        });
        s.apply_event(&AgentEvent::ZoneChanged {
            from: Some(100),
            to: 230,
        });
        assert_eq!(s.zone_id, Some(230));
        assert!(
            s.entities.is_empty(),
            "zone change must clear stale entities"
        );
        assert!(
            s.party.is_empty(),
            "zone change must clear stale party (avoids stale dead-state on home-point warp)"
        );

        // Disconnected lands the terminal stage.
        s.apply_event(&AgentEvent::Disconnected {
            reason: "test".into(),
        });
        assert_eq!(s.stage, Stage::Disconnected);
    }

    #[test]
    fn merge_kind_specialized_wins_over_other() {
        use EntityKind::*;
        // Other never demotes a specialized kind.
        assert_eq!(merge_kind(Pc, Other), Pc);
        assert_eq!(merge_kind(Npc, Other), Npc);
        assert_eq!(merge_kind(Mob, Other), Mob);
        assert_eq!(merge_kind(Pet, Other), Pet);
        // Other → specialized upgrades.
        assert_eq!(merge_kind(Other, Pet), Pet);
        assert_eq!(merge_kind(Other, Npc), Npc);
        // Specialized → specialized: newer wins.
        assert_eq!(merge_kind(Npc, Pet), Pet);
        assert_eq!(merge_kind(Pet, Npc), Npc);
        // Other → Other: no change.
        assert_eq!(merge_kind(Other, Other), Other);
    }

    fn make_test_entity(id: u32, name: Option<&str>, kind: EntityKind) -> Entity {
        Entity {
            id,
            act_index: id as u16,
            kind,
            name: name.map(str::to_string),
            pos: Vec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
        }
    }

    #[test]
    fn entity_upserted_preserves_name_across_attr_only_update() {
        // Spawn with a name (UPDATE_NAME path), then a position-only
        // follow-up arrives with name = None. The cached name must not
        // be lost — this is the bug behind the "? [npc]" target panel.
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(42, Some("Sigli-Sea"), EntityKind::Npc),
        });
        assert_eq!(s.entities[0].name.as_deref(), Some("Sigli-Sea"));

        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(42, None, EntityKind::Npc),
        });
        assert_eq!(
            s.entities[0].name.as_deref(),
            Some("Sigli-Sea"),
            "name must persist across attr-only update"
        );
    }

    #[test]
    fn entity_upserted_preserves_hp_pct_across_position_only_update() {
        // Mirror of the name-preservation test for HP%. The CHAR_NPC
        // producer only fills `hp_pct: Some(_)` when LSB sets UPDATE_HP
        // (0x04) in the updatemask. Position-only ticks arrive with
        // `hp_pct: None`; the reducer must keep the prior value rather
        // than overwriting it with a stale zero byte from the LSB
        // packet buffer.
        let mut s = SessionState::default();
        let mut ent = make_test_entity(42, Some("Worker Bee"), EntityKind::Npc);
        ent.hp_pct = Some(50);
        s.apply_event(&AgentEvent::EntityUpserted { entity: ent });
        assert_eq!(s.entities[0].hp_pct, Some(50));

        let mut pos_only = make_test_entity(42, None, EntityKind::Npc);
        pos_only.hp_pct = None;
        s.apply_event(&AgentEvent::EntityUpserted { entity: pos_only });
        assert_eq!(
            s.entities[0].hp_pct,
            Some(50),
            "hp_pct must persist across UPDATE_POS-only follow-up (no UPDATE_HP bit set)"
        );

        // A genuine UPDATE_HP tick (`Some(0)` = mob just died) must
        // still win — preservation is keyed on the new value being
        // `None`, not on the new value being zero.
        let mut died = make_test_entity(42, None, EntityKind::Npc);
        died.hp_pct = Some(0);
        s.apply_event(&AgentEvent::EntityUpserted { entity: died });
        assert_eq!(
            s.entities[0].hp_pct,
            Some(0),
            "Some(0) (mob died) must overwrite, not get preserved as Some(50)"
        );
    }

    #[test]
    fn entity_upserted_specialized_kind_resists_demotion_to_other() {
        // CHAR_NPC tags entity as Npc; a stray Other update must not
        // demote it. (The 0x067/0x068 phantom-entity bug was the
        // source of Other updates clobbering specialized kinds.)
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(7, Some("Stout Servitor"), EntityKind::Npc),
        });
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(7, Some("Stout Servitor"), EntityKind::Other),
        });
        assert_eq!(s.entities[0].kind, EntityKind::Npc);
    }

    #[test]
    fn entity_patched_by_id_sets_name_on_existing_entity() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(99, None, EntityKind::Other),
        });
        s.apply_event(&AgentEvent::EntityPatched {
            id: Some(99),
            act_index: None,
            name: Some("Mihli Aliapoh".into()),
            kind: Some(EntityKind::Pet),
            hp_pct: None,
        });
        assert_eq!(s.entities[0].name.as_deref(), Some("Mihli Aliapoh"));
        assert_eq!(s.entities[0].kind, EntityKind::Pet);
    }

    #[test]
    fn entity_patched_by_act_index_resolves_when_id_unknown() {
        // PetSync delivers `pet_targid` (act_index) but never the pet's
        // full id — the patch must resolve via act_index.
        let mut s = SessionState::default();
        let mut ent = make_test_entity(0xABCD, None, EntityKind::Other);
        ent.act_index = 0x07A5;
        s.apply_event(&AgentEvent::EntityUpserted { entity: ent });
        s.apply_event(&AgentEvent::EntityPatched {
            id: None,
            act_index: Some(0x07A5),
            name: Some("Crab Familiar".into()),
            kind: Some(EntityKind::Pet),
            hp_pct: Some(75),
        });
        assert_eq!(s.entities[0].name.as_deref(), Some("Crab Familiar"));
        assert_eq!(s.entities[0].kind, EntityKind::Pet);
        assert_eq!(s.entities[0].hp_pct, Some(75));
    }

    #[test]
    fn name_extraction_miss_appends_to_ring_buffer_with_cap() {
        let mut s = SessionState::default();
        // Push more than the cap and verify FIFO eviction.
        for i in 0..(NAME_MISSES_CAP as u32 + 5) {
            s.apply_event(&AgentEvent::NameExtractionMiss {
                miss: NameExtractionMiss {
                    opcode: 0x00E,
                    unique_no: i,
                    act_index: i as u16,
                    send_flag: 0,
                    body_len: 64,
                    body_hex: format!("{:02x}", i & 0xFF),
                    miss_kind: NameMissKind::NameBitClear,
                    at_unix_ms: 1000 + u64::from(i),
                },
            });
        }
        assert_eq!(s.name_misses.len(), NAME_MISSES_CAP);
        // The first 5 entries should have been evicted; oldest now is i=5.
        assert_eq!(s.name_misses.front().unwrap().unique_no, 5);
        // Newest at the back.
        assert_eq!(
            s.name_misses.back().unwrap().unique_no,
            NAME_MISSES_CAP as u32 + 4
        );
    }

    #[test]
    fn name_extraction_miss_round_trips_serde() {
        let miss = NameExtractionMiss {
            opcode: 0x00D,
            unique_no: 0x0102_0304,
            act_index: 0x07A5,
            send_flag: 0x09,
            body_len: 72,
            body_hex: "deadbeef".into(),
            miss_kind: NameMissKind::NameBitSetExtractionFailed,
            at_unix_ms: 1_700_000_000_123,
        };
        let s = serde_json::to_string(&miss).unwrap();
        let back: NameExtractionMiss = serde_json::from_str(&s).unwrap();
        assert_eq!(back.unique_no, 0x0102_0304);
        assert_eq!(back.miss_kind, NameMissKind::NameBitSetExtractionFailed);
        // miss_kind serialises in snake_case to match the AgentEvent tag style.
        assert!(s.contains("name_bit_set_extraction_failed"));
    }

    #[test]
    fn entity_patched_for_unknown_entity_is_dropped() {
        // Without a prior CHAR_NPC spawn there's nothing to patch.
        // Dropping is safe — the next CHAR_NPC will create the entry
        // and a subsequent patch can re-enrich.
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::EntityPatched {
            id: Some(1234),
            act_index: None,
            name: Some("Ghost".into()),
            kind: Some(EntityKind::Pet),
            hp_pct: None,
        });
        assert!(s.entities.is_empty());
    }

    #[test]
    fn heal_command_roundtrip() {
        // MCP / agent harness sends Heal commands as JSON lines, so the
        // snake_case-tagged enum needs to deserialize each mode cleanly.
        for (line, expect) in [
            (r#"{"cmd":"heal","mode":"toggle"}"#, HealMode::Toggle),
            (r#"{"cmd":"heal","mode":"on"}"#, HealMode::On),
            (r#"{"cmd":"heal","mode":"off"}"#, HealMode::Off),
        ] {
            let cmd: AgentCommand = serde_json::from_str(line).unwrap();
            match cmd {
                AgentCommand::Heal { mode } => assert_eq!(mode, expect, "for line {line}"),
                _ => panic!("wrong variant for {line}: {cmd:?}"),
            }
        }
    }

    #[test]
    fn use_item_command_roundtrip() {
        let line = r#"{"cmd":"use_item","container":0,"slot":3,"item_no":4112,"target_id":42,"target_index":7}"#;
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        match cmd {
            AgentCommand::UseItem {
                container,
                slot,
                item_no,
                target_id,
                target_index,
            } => {
                assert_eq!(
                    (container, slot, item_no, target_id, target_index),
                    (0, 3, 4112, 42, 7)
                );
            }
            _ => panic!("wrong variant: {cmd:?}"),
        }
    }

    #[test]
    fn bank_when_full_command_roundtrip() {
        let line = r#"{"cmd":"bank_when_full","threshold":90,"mog_house_zoneline":12345}"#;
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        match cmd {
            AgentCommand::BankWhenFull {
                threshold,
                mog_house_zoneline,
            } => {
                assert_eq!((threshold, mog_house_zoneline), (90, 12345));
            }
            _ => panic!("wrong variant: {cmd:?}"),
        }
    }

    #[test]
    fn reactor_goal_changed_event_roundtrip() {
        let ev = AgentEvent::ReactorGoalChanged {
            goal: ReactorGoalSnapshot::Engaged {
                target_id: 99,
                attack_issued: true,
            },
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: AgentEvent = serde_json::from_str(&s).unwrap();
        match back {
            AgentEvent::ReactorGoalChanged {
                goal:
                    ReactorGoalSnapshot::Engaged {
                        target_id,
                        attack_issued,
                    },
            } => {
                assert_eq!(target_id, 99);
                assert!(attack_issued);
            }
            other => panic!("wrong shape: {other:?}"),
        }
    }

    #[test]
    fn reconnected_fold_writes_last_reconnect() {
        let mut s = SessionState::default();
        assert!(s.last_reconnect.is_none());
        s.apply_event(&AgentEvent::Reconnected { downtime_ms: 1234 });
        let info = s.last_reconnect.expect("set");
        assert_eq!(info.downtime_ms, 1234);
        assert!(info.at_unix_ms > 0, "wall-clock stamped");
    }

    #[test]
    fn self_position_returns_self_entity_pos() {
        let mut s = SessionState::default();
        // No char_id yet → no self position.
        assert!(s.self_position().is_none());

        s.apply_event(&AgentEvent::Connected {
            account_id: 1,
            char_id: 99,
            character: "Self".into(),
            zone_id: 230,
        });
        // char_id known, but no entity yet.
        assert!(s.self_position().is_none());

        // Upsert a self entity; self_position() now returns its fields.
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 99,
                act_index: 5,
                kind: EntityKind::Pc,
                name: Some("Self".into()),
                pos: Vec3 {
                    x: 10.0,
                    y: 20.0,
                    z: 30.0,
                },
                heading: 64,
                hp_pct: Some(100),
                bt_target_id: 0,
                claim_id: 0,
                speed: 40,
                speed_base: 40,
                look: None,
            },
        });
        let p = s.self_position().expect("self entity present");
        assert_eq!(
            p.pos,
            Vec3 {
                x: 10.0,
                y: 20.0,
                z: 30.0
            }
        );
        assert_eq!(p.heading, 64);
        assert_eq!(p.speed, 40);
        assert_eq!(p.speed_base, 40);

        // PositionChanged now also mutates the self entity.
        s.apply_event(&AgentEvent::PositionChanged {
            pos: Position {
                pos: Vec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                heading: 32,
                speed: 25,
                speed_base: 25,
            },
        });
        let p = s.self_position().expect("self entity present");
        assert_eq!(
            p.pos,
            Vec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0
            }
        );
        assert_eq!(p.heading, 32);
    }

    #[test]
    fn reactor_goal_changed_fold_writes_current_goal() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::ReactorGoalChanged {
            goal: ReactorGoalSnapshot::Following {
                target_id: 42,
                distance: 3.0,
            },
        });
        match s.current_goal {
            Some(ReactorGoalSnapshot::Following {
                target_id,
                distance,
            }) => {
                assert_eq!(target_id, 42);
                assert!((distance - 3.0).abs() < 1e-3);
            }
            other => panic!("expected Following, got {other:?}"),
        }
    }

    #[test]
    fn llm_decision_fold_appends_and_caps() {
        let mut s = SessionState::default();
        for i in 0..(RECENT_DECISIONS_CAP + 5) {
            s.apply_event(&AgentEvent::LlmDecision {
                decision: LlmDecision {
                    kind: LlmDecisionKind::ToolDispatched {
                        tool: format!("t{i}"),
                    },
                    latency_us: i as u64,
                    at_monotonic_ms: i as u64,
                },
            });
        }
        assert_eq!(s.recent_decisions.len(), RECENT_DECISIONS_CAP);
        // Oldest 5 dropped.
        match &s.recent_decisions.front().unwrap().kind {
            LlmDecisionKind::ToolDispatched { tool } => assert_eq!(tool, "t5"),
            other => panic!("wrong kind: {other:?}"),
        }
    }

    #[test]
    fn inventory_ready_sets_all_loaded() {
        let mut s = SessionState::default();
        assert!(!s.inventory.all_loaded);
        s.apply_event(&AgentEvent::InventoryReady);
        assert!(s.inventory.all_loaded);
    }

    #[test]
    fn inventory_fold_capacities_writes_each_container() {
        let mut s = SessionState::default();
        let mut caps = vec![0u16; 18];
        caps[0] = 80; // LOC_INVENTORY
        caps[1] = 200; // LOC_MOGSAFE
        caps[5] = 30; // LOC_MOGSATCHEL
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0, // unused for Capacities
            update: InventoryUpdate::Capacities { capacities: caps },
        });
        assert_eq!(s.inventory.containers[&0].capacity, 80);
        assert_eq!(s.inventory.containers[&1].capacity, 200);
        assert_eq!(s.inventory.containers[&5].capacity, 30);
        assert!(
            !s.inventory.containers.contains_key(&7),
            "zero-capacity entries skipped"
        );
    }

    #[test]
    fn inventory_fold_slot_changed_inserts_then_updates_then_removes() {
        let mut s = SessionState::default();
        let slot = ItemSlot {
            index: 3,
            item_no: 4112,
            quantity: 5,
            locked: false,
            price: 0,
        };
        // Insert.
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::SlotChanged { slot: slot.clone() },
        });
        assert_eq!(s.inventory.containers[&0].slots.len(), 1);
        assert_eq!(s.inventory.containers[&0].slots[0].quantity, 5);

        // Update same index.
        let mut updated = slot.clone();
        updated.quantity = 12;
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::SlotChanged { slot: updated },
        });
        assert_eq!(s.inventory.containers[&0].slots.len(), 1, "no duplication");
        assert_eq!(s.inventory.containers[&0].slots[0].quantity, 12);

        // Quantity 0 removes.
        let mut removed = slot.clone();
        removed.quantity = 0;
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::SlotChanged { slot: removed },
        });
        assert!(s.inventory.containers[&0].slots.is_empty());
    }

    #[test]
    fn inventory_fold_quantity_changed_updates_existing_slot_only() {
        let mut s = SessionState::default();
        // Race condition: ITEM_NUM arrives before ITEM_LIST.
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::QuantityChanged {
                index: 7,
                quantity: 99,
                locked: false,
            },
        });
        // Should have been silently dropped — no slot to update.
        assert!(
            s.inventory
                .containers
                .get(&0)
                .map(|c| c.slots.is_empty())
                .unwrap_or(true),
            "ITEM_NUM without prior ITEM_LIST drops"
        );

        // Now seed via SlotChanged, then ITEM_NUM updates the qty.
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::SlotChanged {
                slot: ItemSlot {
                    index: 7,
                    item_no: 4112,
                    quantity: 1,
                    locked: false,
                    price: 0,
                },
            },
        });
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::QuantityChanged {
                index: 7,
                quantity: 25,
                locked: true,
            },
        });
        let slot = &s.inventory.containers[&0].slots[0];
        assert_eq!(slot.quantity, 25);
        assert!(slot.locked, "lock flag updated");
        assert_eq!(slot.item_no, 4112, "item_no preserved (qty-only update)");
    }

    #[test]
    fn apply_event_caps_chat_history() {
        let mut s = SessionState::default();
        for i in 0..(CHAT_HISTORY_CAP + 50) {
            s.apply_event(&AgentEvent::ChatLine {
                line: ChatLine {
                    channel: ChatChannel::Say,
                    sender: "x".into(),
                    text: format!("msg {i}"),
                    server_ts: 0,
                },
            });
        }
        assert_eq!(s.chat.len(), CHAT_HISTORY_CAP);
        // The oldest 50 should have been dropped.
        assert_eq!(s.chat[0].text, "msg 50");
    }
}
