use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub fn process_monotonic_ms() -> u64 {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    let start = ANCHOR.get_or_init(Instant::now);
    start.elapsed().as_millis() as u64
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Stage {
    #[default]
    Idle,
    Authenticating,
    LobbyHandshake,
    MapBootstrap,
    Zoning,
    InZone,
    Disconnected,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub pos: Vec3,

    pub heading: u8,

    #[serde(default = "default_speed")]
    pub speed: u8,

    #[serde(default = "default_speed")]
    pub speed_base: u8,
}

fn default_speed() -> u8 {
    25
}

fn default_fps() -> u32 {
    60
}

fn default_true() -> bool {
    true
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

#[inline]
pub fn heading_to_forward(heading: u8) -> (f32, f32) {
    let angle = (heading as f32) * std::f32::consts::TAU / 256.0;
    (angle.cos(), -angle.sin())
}

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

pub const MODEL_RADIUS_PC: f32 = 0.35;
pub const MODEL_RADIUS_NPC: f32 = 0.5;
pub const MODEL_RADIUS_MOB: f32 = 0.55;
pub const MODEL_RADIUS_PET: f32 = 0.4;
pub const MODEL_RADIUS_OTHER: f32 = 0.5;

pub const CONTACT_GAP: f32 = 0.0;

pub fn model_radius(kind: EntityKind) -> f32 {
    match kind {
        EntityKind::Pc => MODEL_RADIUS_PC,
        EntityKind::Npc => MODEL_RADIUS_NPC,
        EntityKind::Mob => MODEL_RADIUS_MOB,
        EntityKind::Pet => MODEL_RADIUS_PET,
        EntityKind::Other => MODEL_RADIUS_OTHER,
    }
}

/// Retail's own decode of the wire speed byte
/// (research/XIClient .../Game/Net/Packets/s2c, RecvCharPc and RecvServerStatus).
pub const SPEED_TO_YPS: f32 = 0.1;

// The server does not send a faster speed to a mounted player — LSB caps its
// mount speed at map.MOUNT_SPEED/2 = 40, *below* the 50 it sends on foot
// (vendor/server/src/map/entities/battleentity.cpp, CBattleEntity::UpdateSpeed).
// Retail makes up the difference in the client, doubling the decoded speed while
// mounted and then clamping
// (research/XIClient .../World/Actor/ControllableActor.cpp,
// ControllableActor::StepControl). Taking the packet at face value therefore
// makes mounting *slower*.
pub const MOUNTED_SPEED_MULTIPLIER: f32 = 2.0;
pub const MAX_MOVE_SPEED_YPS: f32 = 30.0;

/// The speed LSB sends an unmounted PC, which every "step per tick" budget in
/// the reactor is calibrated against
/// (vendor/server/src/map/entities/battleentity.cpp, CBattleEntity::UpdateSpeed).
pub const BASE_PACKET_SPEED: u8 = 50;

/// Yalms per second for a decoded packet speed. `speed_base` is a separate value
/// retail keeps but never spends on the movement rate — `StepControl` reads only
/// the doubled-and-clamped `speed`, so scaling by `speed / speed_base` would
/// under-drive a mounted PC rather than over-drive it.
pub const fn move_speed_yps(packet_speed: u8, mounted: bool) -> f32 {
    let speed = packet_speed as f32 * SPEED_TO_YPS;
    let speed = if mounted {
        speed * MOUNTED_SPEED_MULTIPLIER
    } else {
        speed
    };
    speed.min(MAX_MOVE_SPEED_YPS)
}

/// Movement rate as a multiple of the unmounted run the callers' per-tick step
/// budgets are sized for.
pub fn move_speed_ratio(packet_speed: u8, mounted: bool) -> f32 {
    move_speed_yps(packet_speed, mounted) / move_speed_yps(BASE_PACKET_SPEED, false)
}

fn merge_kind(existing: EntityKind, incoming: EntityKind) -> EntityKind {
    use EntityKind::*;
    match (existing, incoming) {
        (Pc | Npc | Mob | Pet, Other) => existing,
        _ => incoming,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: u32,

    pub act_index: u16,
    pub kind: EntityKind,
    pub name: Option<String>,
    pub pos: Vec3,
    pub heading: u8,

    pub hp_pct: Option<u8>,

    #[serde(default)]
    pub bt_target_id: u32,

    /// Head-look target: the targid (act_index) this entity has selected, from
    /// PosHead Flags0. Drives the head turn other clients see. Lives in the
    /// Position block, so preserved across non-position updates (like `pos`).
    #[serde(default)]
    pub face_target: u16,

    /// entity_update namevis byte (PosHead flags3 top byte), written under
    /// UPDATE_HP — vendor/server/src/map/packets/entity_update.cpp:357/:408 put
    /// `ref<uint8>(0x2B) = PEntity->namevis` inside `if (updatemask & UPDATE_HP)`.
    /// The packet buffer is zero-filled, so a POS-only update carries no namevis:
    /// `None` until the first General-block update does, preserved across
    /// pos-only updates like `char_flags`.
    #[serde(default)]
    pub name_vis: Option<u8>,

    #[serde(default)]
    pub claim_id: u32,

    #[serde(default)]
    pub speed: u8,

    #[serde(default)]
    pub speed_base: u8,

    #[serde(skip)]
    pub look: Option<ffxi_proto::decode::LookData>,

    /// NPC animation/animationsub; `animationsub != 0` marks effect NPCs
    /// (brazier/lamp/torch flames). Preserved across pos-only updates like `look`.
    #[serde(skip)]
    pub npc_state: Option<ffxi_proto::decode::NpcState>,

    /// `Flags1`/`Flags2`/`Flags3` of the last General-block update. Drives the
    /// retail nameplate colour and icon markers. Preserved across pos-only
    /// updates like `look`, since the server only refreshes the words when the
    /// General send-flag bit is set.
    #[serde(skip)]
    pub char_flags: Option<ffxi_proto::decode::CharFlags>,

    /// Live LSB STATUS_TYPE byte, refreshed every update (the server writes it
    /// unconditionally, unlike npc_state's UPDATE_HP-gated fields). 0 = NORMAL.
    /// Authoritative for target eligibility; see `kuluu_snapshot::Entity`.
    #[serde(default)]
    pub status: u8,

    /// `Flags6.MountIndex` of the last General-block update. Says which mount,
    /// never whether one is being ridden — that is `npc_state.animation`.
    /// Preserved across pos-only updates like `look`, for the same reason.
    #[serde(skip)]
    pub mount_id: Option<u8>,
}

/// Which retail colour a run of a chat line takes. Retail renders some
/// substitutions apart from the text around them — the item name in
/// "You find a [lizard tail] on the Rock Lizard." is green against the rest
/// (`.agents/skills/retail-observe/references/treasure-pool-chat.md`).
/// Projected to `kuluu_snapshot::ChatSpanKind` by `wire_translate`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatSpanKind {
    Text,
    Item,
    KeyItem,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChatSpan {
    pub text: String,
    pub kind: ChatSpanKind,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatLine {
    pub channel: ChatChannel,
    pub sender: String,
    pub text: String,

    pub server_ts: u32,

    /// Per-substitution colouring, for the lines retail renders multicoloured.
    /// Empty means the whole line takes the channel colour; when set, the
    /// concatenated span text equals `text`.
    #[serde(default)]
    pub spans: Vec<ChatSpan>,
}

/// Whether the local player has acted on a pool item
/// (`GC_ITEM_TROPHY_ENTRY_KIND`, s2c 0x0D2 `Entry`).
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreasureEntry {
    #[default]
    None,
    Passed,
    Lotted,
}

/// One occupied treasure-pool slot. `start_time` is the server's own clock
/// reading at the drop (s2c 0x0D2 `StartTime`), so it is only meaningful as a
/// difference against later drops, not as a wall clock.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreasurePoolSlot {
    pub slot: u8,
    pub item_id: u16,
    pub item_name: String,
    pub count: u32,
    pub dropper: String,
    pub start_time: u32,
    pub own_entry: TreasureEntry,
    pub own_lot: Option<u16>,
    pub winner: Option<String>,
    pub winner_lot: u16,
}

impl ChatLine {
    /// A line with no per-substitution colouring — the common case.
    pub fn plain(channel: ChatChannel, sender: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            channel,
            sender: sender.into(),
            text: text.into(),
            ..Default::default()
        }
    }

    /// A line whose spans carry retail's colouring; `text` is kept as their
    /// concatenation so every plain-text consumer still reads the whole line.
    pub fn spanned(channel: ChatChannel, spans: Vec<ChatSpan>) -> Self {
        Self {
            channel,
            text: spans.iter().map(|s| s.text.as_str()).collect(),
            spans,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatChannel {
    Say,
    Shout,
    Tell,
    Party,
    Linkshell,
    Yell,
    System,
    /// The catch-all for chat kinds with no dedicated channel, and so the
    /// default a partially-built line starts from.
    #[default]
    Other,

    Battle,

    Debug,

    /// Chat kind 8 MESSAGE_EMOTION: canned-emote lines the client composes
    /// from its DAT, plus free-form /em text
    /// (vendor/server/src/map/enums/chat_message_type.h:35).
    Emote,
}

impl ChatChannel {
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

            k::EMOTION => Self::Emote,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub stage: Option<Stage>,
    pub blowfish_status: Option<BlowfishStatus>,
    pub sync_in: Option<u16>,
    pub sync_out: Option<u16>,

    pub last_server_packet_age_ms: Option<u64>,

    pub cert_sha256: Option<String>,
    pub map_server_addr: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetStats {
    pub send_bps: u32,
    pub recv_bps: u32,
    pub send_health: u8,
    pub recv_health: u8,
}

/// Self-character stat block, folded from s2c 0x061 (CLISTATUS). `bonus`/`resist`
/// are signed gear/buff deltas; `ilvl` is the amount above 99 the server sends
/// (0 when the character has no item-level gear).
/// See ffxi_proto::decode::CliStatus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharStatsRaw {
    pub hp_max: u32,
    pub mp_max: u32,
    pub bp_base: [u16; 7],
    pub bonus: [i16; 7],
    pub attack: u16,
    pub defense: u16,
    pub resist: [i16; 8],
    pub ilvl: u8,
}

/// s2c 0x00A myroom cluster; present only while inside a Mog House. `model`
/// is an interior model id, not a zone id
/// (vendor/server/src/map/packets/s2c/0x00a_login.cpp:32-34).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyRoomInfo {
    pub model: u16,
    pub sub_map: u8,
    pub exit_bit: u8,
}

/// s2c 0x01B JOB_INFO; `job_levels` is indexed by JOBTYPE and `unlocked` bit 0
/// is the subjob-feature flag, not a job
/// (vendor/server/src/map/packets/s2c/0x01b_job_info.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobInfoState {
    pub mjob_no: u8,
    pub sjob_no: u8,
    pub unlocked: u32,
    pub sub_job_unlocked: bool,
    pub job_levels: [u8; ffxi_proto::decode::JobInfo::MAX_JOBTYPE],
}

impl From<ffxi_proto::decode::JobInfo> for JobInfoState {
    fn from(j: ffxi_proto::decode::JobInfo) -> Self {
        Self {
            mjob_no: j.mjob_no,
            sjob_no: j.sjob_no,
            unlocked: j.unlocked,
            sub_job_unlocked: j.sub_job_unlocked,
            job_levels: j.job_levels,
        }
    }
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

    /// Monotonically increasing counter, bumped on every `ZoneChanged`. The
    /// renderer's party-frame content key includes this so a zone transition
    /// always forces a UI rebuild, even when the party data looks identical.
    #[serde(default)]
    pub zone_generation: u64,

    pub chat: Vec<ChatLine>,

    /// Lines already evicted from `chat` by [`CHAT_HISTORY_CAP`], so
    /// `chat_dropped + i` is the absolute history index of `chat[i]`. The viewer
    /// merges its local toasts against that index; anything derived from the
    /// live position renumbers on every eviction (kuluu-zvc3).
    pub chat_dropped: u64,

    /// The 10 treasure-pool slots, indexed by `TrophyItemIndex`. Cleared on
    /// zone change: a player's lot/pass state only lives as long as they stay
    /// in the zone and party
    /// (research/XiPackets/world/server/0x00D2).
    #[serde(default)]
    pub treasure_pool: Vec<Option<TreasurePoolSlot>>,

    pub diagnostics: Diagnostics,

    #[serde(default)]
    pub net_stats: NetStats,

    #[serde(default)]
    pub inventory: Inventory,

    #[serde(default)]
    pub current_goal: Option<ReactorGoalSnapshot>,

    #[serde(default)]
    pub last_reconnect: Option<ReconnectInfo>,

    #[serde(default = "default_fps")]
    pub target_fps: u32,

    #[serde(default, skip_serializing_if = "VecDeque::is_empty")]
    pub name_misses: VecDeque<NameExtractionMiss>,

    #[serde(default)]
    pub dialog: Option<DialogState>,

    #[serde(default)]
    pub shop: Option<ShopState>,

    #[serde(default)]
    pub status_icons: Vec<u16>,

    #[serde(default)]
    pub status_icon_expiries: Vec<u32>,

    #[serde(default)]
    pub ability_recasts: Vec<(u16, u32)>,

    #[serde(default)]
    pub logout_countdown: Option<LogoutCountdown>,

    #[serde(default)]
    pub death_homepoint_secs: Option<u32>,

    /// Server-offered alternative to returning home while dead (s2c 0x0F9).
    /// `None` is the ordinary home-point-only menu.
    #[serde(default)]
    pub death_menu_offer: Option<ffxi_proto::decode::DeathMenuOffer>,

    #[serde(default)]
    pub current_weather: Option<u16>,

    #[serde(default = "default_equipment")]
    pub equipment: [Option<EquippedRef>; EQUIPMENT_SLOTS],

    #[serde(default)]
    pub char_stats: Option<CharStatsRaw>,

    #[serde(default)]
    pub spells_known: Vec<u16>,

    #[serde(default)]
    pub job_abilities_known: Vec<u16>,

    #[serde(default)]
    pub weaponskills_known: Vec<u16>,

    #[serde(default)]
    pub pet_abilities_known: Vec<u16>,

    #[serde(default)]
    pub key_items: Vec<u16>,

    #[serde(default)]
    pub key_items_seen: Vec<u16>,

    #[serde(default)]
    pub self_fishing: Option<SelfFishing>,

    /// The server's animation byte for self, from 0x037 CHAR_STATUS. Self never
    /// appears in the CHAR_PC stream that carries `Entity::animation` for other
    /// players, so this is the only authority for our own rest state.
    #[serde(default)]
    pub self_server_status: u8,

    /// 0x037's `mount_id`, paired with `self_server_status`: which mount we are on,
    /// while that byte says we are on one at all.
    #[serde(default)]
    pub self_mount_id: u8,

    /// Latched self appearance from 0x00A LOGIN / 0x051 GRAP_LIST. Ordering
    /// proof: 0x051 can land before self's entity exists, and `ZoneChanged`
    /// clears `entities`, so the last-known look is re-applied on upsert.
    #[serde(skip)]
    pub self_look: Option<ffxi_proto::decode::LookData>,

    /// Projection of the reactor's in-flight cast/action for the Enhanced cast
    /// bar. The reactor's `CastInFlight` is the authoritative owner; this is the
    /// serializable view it republishes each tick (mirrors `self_fishing`).
    #[serde(default)]
    pub self_casting: Option<SelfCasting>,

    #[serde(default)]
    pub myroom: Option<MyRoomInfo>,

    /// `SubMapNumber` the server reported in 0x00A LOGIN
    /// (`PChar->loc.boundary`): which sub-area interior the character is
    /// standing in on arrival. Cleared by a zone change, so it only ever
    /// describes the zone currently loaded.
    #[serde(default)]
    pub sub_area: Option<u16>,

    #[serde(default)]
    pub mog_zone_flag: bool,

    #[serde(default)]
    pub job_info: Option<JobInfoState>,

    /// 2F-unlock bit from the self 0x067 CharSync
    /// (vendor/server/src/map/packets/char_sync.cpp:61); `None` until one lands.
    #[serde(default)]
    pub mh_2f_unlocked: Option<bool>,

    /// Job-emote unlock bitfield from s2c 0x11A (bit = job id - 1); `None`
    /// until the server answers a 0x119 EMOTE_LIST request.
    #[serde(default)]
    pub emote_jobs: Option<u32>,

    /// Chair unlock bitfield from s2c 0x11A.
    #[serde(default)]
    pub emote_chairs: Option<u16>,

    #[serde(default)]
    pub delivery_box: DeliveryBoxState,

    #[serde(default)]
    pub check_result: Option<CheckResult>,

    /// s2c 0x0CA answer to the same /check: the target's bazaar message. The
    /// packet carries no target id, only `sName`, so it is kept beside
    /// `check_result` rather than merged into it.
    #[serde(default)]
    pub check_message: Option<CheckMessage>,

    /// The bazaar we are currently browsing (c2s 0x105 → s2c 0x105 rows).
    #[serde(default)]
    pub bazaar: Option<BazaarView>,

    #[serde(default)]
    pub auction: AuctionState,

    /// Server-driven wide-scan (tracking) list, accumulated between the s2c 0x0F6
    /// ListStart/ListEnd frames; `tracked` follows s2c 0x0F5.
    #[serde(default)]
    pub widescan: WidescanList,
}

/// s2c 0x0CA INSPECT_MESSAGE: the checked PC's bazaar/seek message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckMessage {
    pub name: String,
    pub message: String,
}

/// A bazaar being browsed. Rows are keyed by the seller's LOC_INVENTORY slot
/// because the server refreshes single rows in place after each purchase
/// (vendor/server/src/map/packets/c2s/0x106_bazaar_buy.cpp:198).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BazaarView {
    pub seller_id: u32,
    pub seller_index: u16,
    pub seller_name: String,
    pub items: Vec<BazaarItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BazaarItem {
    pub index: u8,
    pub item_no: u16,
    pub quantity: u32,
    /// Seller's asking price per unit, before tax.
    pub price: u32,
    /// Zone tax in hundredths of a percent; the buyer-facing total is computed
    /// where it is displayed (`kuluu_snapshot::BazaarEntry::total_price`).
    pub tax_rate: u16,
}

pub const AUCTION_SLOTS: usize = ffxi_proto::decode::AUCTION_SLOT_COUNT as usize;

/// Auction House model. The counter menu rides s2c 0x04C (map server); the
/// browse catalog and price history come from the search server
/// ([`crate::search_client`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuctionState {
    /// Set by the s2c Open push; the close is client-local, so it only clears
    /// on zone change (the counter NPC is zone-local, like a bazaar seller).
    pub open: bool,
    pub browse: Option<AhCatalogView>,
    pub history: Option<AhHistoryView>,
    pub sales_status: [Option<AhSaleStatus>; AUCTION_SLOTS],
    /// Last AskCommit fee quote; consumed by `AgentCommand::AhSellConfirm`.
    pub fee_quote: Option<AhFeeQuote>,
    pub busy: Option<AuctionBusy>,
}

/// Which retail spinner the in-flight AH op renders ("Downloading data .." /
/// "Placing bid ...", observation record
/// .agents/skills/retail-observe/references/auction-house.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuctionBusy {
    Downloading,
    PlacingBid,
}

/// One category's full catalog (all TCP_AH_REQUEST pages merged).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhCatalogView {
    pub category: u8,
    pub total: u16,
    pub listings: Vec<AhListingView>,
}

/// Serde mirror of `ffxi_proto::search::AhListing` — open-listing counts (the
/// retail catalog's bracketed `[N]` stock numbers), never prices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhListingView {
    pub item_id: u16,
    /// Singles currently up for sale; 0 = none listed.
    pub singles_for_sale: u32,
    /// Stacks currently up for sale; `None` = item is not stackable.
    pub stacks_for_sale: Option<u32>,
}

impl From<ffxi_proto::search::AhListing> for AhListingView {
    fn from(l: ffxi_proto::search::AhListing) -> Self {
        Self {
            item_id: l.item_id,
            singles_for_sale: l.singles_for_sale,
            stacks_for_sale: l.stacks_for_sale,
        }
    }
}

/// Serde mirror of `ffxi_proto::search::AhHistory` plus the request's
/// single-vs-stack form, which the response does not echo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhHistoryView {
    pub item_id: u16,
    pub stack: bool,
    /// Count of open listings of the requested form; not a price.
    pub open_listings: u32,
    pub category: u16,
    pub sales: Vec<AhSaleView>,
}

impl AhHistoryView {
    pub fn from_wire(h: ffxi_proto::search::AhHistory, stack: bool) -> Self {
        Self {
            item_id: h.item_id,
            stack,
            open_listings: h.open_listings,
            category: h.category,
            sales: h.sales.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhSaleView {
    pub price: u32,
    pub sell_date: u32,
    pub seller: String,
    pub buyer: String,
}

impl From<ffxi_proto::search::AhSale> for AhSaleView {
    fn from(s: ffxi_proto::search::AhSale) -> Self {
        Self {
            price: s.price,
            sell_date: s.sell_date,
            seller: s.seller,
            buyer: s.buyer,
        }
    }
}

/// One populated sales-status slot. Serde mirror of the
/// `ffxi_proto::decode::AuctionSaleSlot` Parcel fields the client uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhSaleStatus {
    pub stat: u8,
    pub item_no: u16,
    /// 1 for a single, the stack size for a stack (GP_SERV_COMMAND_AUC slot
    /// constructor writes `1 - stack`).
    pub quantity: u8,
    pub price: u32,
    pub timestamp: u32,
}

impl From<&ffxi_proto::decode::AuctionSaleSlot> for AhSaleStatus {
    fn from(s: &ffxi_proto::decode::AuctionSaleSlot) -> Self {
        Self {
            stat: s.stat,
            item_no: s.item_no,
            quantity: s.quantity,
            price: s.price,
            timestamp: s.timestamp,
        }
    }
}

/// The s2c AskCommit fee quote plus the asking price the session sent, which
/// the quote does not echo (the CItem GP_SERV_COMMAND_AUC constructor writes
/// the computed fee into Commission).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhFeeQuote {
    pub fee: u32,
    pub inventory_slot: u8,
    pub item_no: u16,
    pub stack: bool,
    pub asking_price: u32,
}

/// Faithful wide-scan model: the server owns membership, order, and gating
/// (job/range/floor — vendor/server/src/map/zone_entities.cpp:1578 WideScan).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WidescanList {
    pub entries: Vec<WidescanEntry>,
    pub tracked: Option<WidescanPos>,
    /// `true` while entries are still arriving (between 0x0F6 ListStart and ListEnd).
    pub building: bool,
}

/// One wide-scan list row. Serde mirror of `ffxi_proto::decode::WidescanEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidescanEntry {
    pub act_index: u16,
    pub level: u8,
    /// Marker category: 0 = char, 1 = npc, 2 = mob (0x0f4_tracking_list.cpp).
    pub kind: u8,
    /// Entity minus self position in server units.
    pub rel_x: i16,
    pub rel_z: i16,
    /// Server sName, or — when sent empty (0x0f4_tracking_list.cpp TODO) — the
    /// zone NPC-name DAT name for `act_index`, resolved at decode like retail
    /// (research/XiPackets world/server/0x00F4). The viewer still falls back to
    /// the local entity name.
    pub name: String,
}

impl From<ffxi_proto::decode::WidescanEntry> for WidescanEntry {
    fn from(e: ffxi_proto::decode::WidescanEntry) -> Self {
        Self {
            act_index: e.act_index,
            level: e.level,
            kind: e.kind,
            rel_x: e.rel_x,
            rel_z: e.rel_z,
            name: e.name,
        }
    }
}

/// Tracked-entity absolute position. Serde mirror of
/// `ffxi_proto::decode::WidescanPos` (raw server coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WidescanPos {
    pub act_index: u16,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl From<ffxi_proto::decode::WidescanPos> for WidescanPos {
    fn from(p: ffxi_proto::decode::WidescanPos) -> Self {
        Self {
            act_index: p.act_index,
            x: p.x,
            y: p.y,
            z: p.z,
        }
    }
}

/// Accumulated s2c 0x0C9 EQUIP_INSPECT answer for the latest /check on a PC:
/// EQUIPMENT batches and the GENERAL packet merge here keyed on `target_id`
/// (vendor/server/src/map/packets/c2s/0x0dd_equip_inspect.cpp:135-136).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub target_id: u32,
    pub act_index: u16,
    #[serde(default = "default_check_equipped")]
    pub equipped: [Option<u16>; EQUIPMENT_SLOTS],
    pub main_job: u8,
    pub sub_job: u8,
    pub main_job_lv: u8,
    pub sub_job_lv: u8,
    pub master_lv: u8,
    /// Equipped linkshell's name, unpacked from the GENERAL packet's 6-bit
    /// `sComLinkName`; empty when the target wears no pearl.
    #[serde(default)]
    pub linkshell: String,
}

impl CheckResult {
    fn new(target_id: u32, act_index: u16) -> Self {
        Self {
            target_id,
            act_index,
            equipped: default_check_equipped(),
            main_job: 0,
            sub_job: 0,
            main_job_lv: 0,
            sub_job_lv: 0,
            master_lv: 0,
            linkshell: String::new(),
        }
    }
}

fn default_check_equipped() -> [Option<u16>; EQUIPMENT_SLOTS] {
    [None; EQUIPMENT_SLOTS]
}

pub const EQUIPMENT_SLOTS: usize = 16;

pub const KEY_ITEMS_PER_TABLE: usize = ffxi_proto::decode::ScenarioItem::BITS_PER_TABLE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquippedRef {
    pub container: u8,
    pub container_index: u8,
}

fn default_equipment() -> [Option<EquippedRef>; EQUIPMENT_SLOTS] {
    [None; EQUIPMENT_SLOTS]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LogoutCountdown {
    pub seconds_remaining: u16,

    pub shutdown: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DialogState {
    pub event_id: u32,
    pub npc_id: u32,

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
    /// Event-VM-rendered NPC speech (real dialog text); `None` on the raw-packet
    /// fallback path (when no event DAT could drive the dialog).
    #[serde(default)]
    pub prompt: Option<String>,
    /// Event-VM-rendered menu option labels for a choice frame.
    #[serde(default)]
    pub choices: Vec<String>,
    /// Free-text entry frame (delivery-box recipient prompt): the viewer
    /// answers with `AgentCommand::TextInput` instead of a menu choice.
    #[serde(default)]
    pub text_entry: bool,
    /// Grid presentation metadata for a choice frame (delivery-box 2x4 slot
    /// grid). Cells are row-major; each active cell maps back to an index in
    /// `choices`, so answering works identically to a plain list frame.
    /// Choices not referenced by any cell (recipient row, Close) render as
    /// ordinary list rows around the grid.
    #[serde(default)]
    pub grid: Option<DialogGrid>,
    /// Server customMenu (GMPROMPT/`_CUSTOM_MENU`) prompt rather than an event-VM
    /// or client-local frame. A selection round-trips as a `_CUSTOM_MENU` tell
    /// (`AgentCommand::CustomMenuRespond`), not an `EndEventChoice`.
    #[serde(default)]
    pub custom_menu: bool,
}

/// Row-major grid overlay for a choice frame (`cells.len() == rows * cols`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DialogGrid {
    pub cols: u8,
    pub rows: u8,
    pub cells: Vec<DialogGridCell>,
}

/// One cell of a [`DialogGrid`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DialogGridCell {
    /// Index into `DialogState::choices` this cell activates; `None` for an
    /// inert cell (empty slot that is not currently selectable).
    pub choice: Option<u32>,
    /// Item occupying the cell, if any (drives the icon in the viewer).
    pub item_no: Option<u16>,
    pub quantity: u32,
    /// True once an outgoing item has been dispatched to the server
    /// ("(sent)" vs "(preparing)" in the flat label).
    pub sent: bool,
}

/// Which entity a [`CutsceneCue`] names, resolved from the event VM's
/// [`ffxi_event::ActorLookup`] against the running event's own entity (the VM
/// deliberately leaves that to its host).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutsceneActor {
    LocalPlayer,
    Entity { server_id: u32 },
}

/// One staging effect the running event script asked for — the renderer-facing
/// half of [`ffxi_event::EventCue`]. `MusicVolume` is absent because 0x5D rides
/// [`AgentEvent::MusicVolumeChanged`] instead of the cue stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CutsceneCue {
    ActorMotion {
        actor: CutsceneActor,
        partner: CutsceneActor,
        key: ffxi_event::FourCc,
    },
    Scheduler {
        dat_id: u32,
        actor: CutsceneActor,
        partner: CutsceneActor,
        tag: ffxi_event::FourCc,
        duration: u16,
    },
    ActorHide {
        target: CutsceneActor,
        hide: bool,
    },
    CameraLock {
        lock: bool,
    },
    Mount {
        target: CutsceneActor,
        status_event: u8,
        mount_id: Option<u16>,
    },
}

/// Number of music slots [`AgentEvent::MusicVolumeChanged::slot`] can name
/// (vendor/server/src/map/enums/music_slot.h `MusicSlot`, ZoneDay..Fishing).
/// The 0x5D event opcode sets retail's single master music volume, so it is
/// carried as the same volume on every slot.
pub const MUSIC_SLOT_COUNT: u8 = 8;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Inventory {
    pub containers: HashMap<u8, ContainerInfo>,

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
    #[serde(default)]
    pub charges_remaining: Option<u8>,
    #[serde(default)]
    pub next_use_vana_ts: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InventoryUpdate {
    Capacities {
        capacities: Vec<u16>,
    },

    SlotChanged {
        slot: ItemSlot,
    },

    QuantityChanged {
        index: u8,
        quantity: u32,
        locked: bool,
    },
}

/// GP_CLI_COMMAND_PBX_BOXNO (vendor/server/src/map/packets/c2s/0x04d_pbx.h:45).
/// Incoming = the inbox ("Delivery Box"), Outgoing = the send box ("Deliveries").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryBoxNo {
    Incoming,
    Outgoing,
}

impl DeliveryBoxNo {
    pub fn wire(self) -> i8 {
        match self {
            DeliveryBoxNo::Incoming => ffxi_proto::map::pbx::boxno::INCOMING,
            DeliveryBoxNo::Outgoing => ffxi_proto::map::pbx::boxno::OUTGOING,
        }
    }

    pub fn from_wire(v: i8) -> Option<Self> {
        match v {
            v if v == ffxi_proto::map::pbx::boxno::INCOMING => Some(DeliveryBoxNo::Incoming),
            v if v == ffxi_proto::map::pbx::boxno::OUTGOING => Some(DeliveryBoxNo::Outgoing),
            _ => None,
        }
    }
}

/// One c2s 0x04D PBX request, named 1:1 after GP_CLI_COMMAND_PBX_COMMAND
/// (vendor/server/src/map/packets/c2s/0x04d_pbx.h). Fields carry only what
/// LSB's PacketValidator lets vary per command; everything else is fixed by
/// the encoder ([`crate::session::build_subpacket_pbx`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DeliveryBoxOp {
    /// List a box's slots; the server replies with 8 per-slot Work results.
    Work { box_no: DeliveryBoxNo },
    /// Stage `quantity` of the LOC_INVENTORY item at `inventory_slot` into
    /// outbox slot `slot`, addressed to `recipient`.
    Set {
        slot: u8,
        inventory_slot: u8,
        quantity: u32,
        recipient: String,
    },
    /// Dispatch the staged item in outbox slot `slot`.
    Send { slot: u8 },
    /// Cancel the dispatched (not yet received) item in outbox slot `slot`.
    Cancel { slot: u8 },
    /// Ask for the new/delivered item count (answered in ResParam2/3).
    Check { box_no: DeliveryBoxNo },
    /// Move the oldest queued incoming item into inbox slot `slot`.
    Recv { slot: u8 },
    /// Remove the oldest delivered item from the outbox.
    Confirm,
    /// Select an inbox slot before removal (server echoes the item; retail
    /// sends this ahead of Get). LSB pins its BoxNo to Incoming.
    Accept { slot: u8 },
    /// Return the incoming item in inbox slot `slot` to its sender.
    Reject { slot: u8 },
    /// Take the item in `slot` into LOC_INVENTORY.
    Get { box_no: DeliveryBoxNo, slot: u8 },
    /// Delete the incoming item in inbox slot `slot` without taking it.
    Clear { box_no: DeliveryBoxNo, slot: u8 },
    /// Verify `recipient` names an existing character before staging.
    Query { recipient: String },
    /// Enter delivery (send) mode — opens the outbox server-side.
    DeliOpen,
    /// Enter post (receive) mode — opens the inbox server-side.
    PostOpen,
    /// Exit delivery/post mode.
    PostClose { box_no: DeliveryBoxNo },
}

/// An item occupying a delivery box slot. `counterpart` is the sender
/// (Incoming) or recipient (Outgoing); `stat` is the raw GP_POST_BOX_STATE
/// Stat byte (see ffxi_proto::map::pbx::stat).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryItem {
    pub item_no: u16,
    pub quantity: u32,
    pub counterpart: Option<String>,
    pub stat: u32,
}

impl DeliveryItem {
    pub fn sent(&self) -> bool {
        self.stat == ffxi_proto::map::pbx::stat::SENT
    }
}

/// Resolution state of the outgoing recipient name. Mirrors
/// `kuluu_snapshot::RecipientStatus`; `Ok.same_account` is LSB ResParam1.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecipientStatus {
    #[default]
    Unset,
    Pending,
    Ok {
        same_account: bool,
    },
    NoSuchChar,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeliveryBoxState {
    /// Which box the server currently has open for us, if any.
    pub open: Option<DeliveryBoxNo>,
    pub slots: Vec<Option<DeliveryItem>>,
    /// Last Check answer: items still queued beyond the 8 visible slots.
    pub queued: u8,
    /// Outgoing: the typed/locked recipient name (authoritative here so it
    /// reaches the wire snapshot; `local_menu` mirrors it for the legacy path).
    pub recipient: Option<String>,
    /// Outgoing: recipient name resolution state.
    pub recipient_status: RecipientStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeliveryBoxUpdate {
    Opened,
    Closed,
    SlotChanged {
        slot: u8,
        item: Option<DeliveryItem>,
    },
    /// Check result: new items queued (Incoming) or delivered (Outgoing).
    PendingCount {
        count: u8,
    },
    /// The session just sent a recipient Query: record the name and mark the
    /// resolution pending so the screen shows "(checking…)".
    RecipientPending {
        name: String,
    },
    /// Query result. `ok` = the name resolved to an account (a nonexistent
    /// name answers Result 0xFB instead); `same_account` mirrors LSB's
    /// ResParam1, which is 1 only when the recipient shares the sender's
    /// account (dboxutils.cpp ConfirmNameBeforeSending) — NOT an existence
    /// flag.
    RecipientCheck {
        ok: bool,
        same_account: bool,
    },
    /// A non-OK Result byte (see ffxi_proto::map::pbx::result).
    Failed {
        command: u8,
        result: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[derive(Default)]
pub enum ReactorGoalSnapshot {
    #[default]
    Idle,
    Following {
        target_id: u32,
        distance: f32,
    },
    Engaged {
        target_id: u32,
        attack_issued: bool,
    },

    Pathing {
        x: f32,
        y: f32,
        z: f32,
        #[serde(default = "one_u32")]
        waypoints_remaining: u32,
    },

    Banking {
        threshold: u8,
        mog_house_zoneline: u32,
    },
}

fn one_u32() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectInfo {
    pub downtime_ms: u64,
    pub at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameMissKind {
    NameBitClear,

    NameBitSetExtractionFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameExtractionMiss {
    pub opcode: u16,
    pub unique_no: u32,
    pub act_index: u16,

    pub send_flag: u8,
    pub body_len: usize,

    pub body_hex: String,
    pub miss_kind: NameMissKind,

    pub at_unix_ms: u64,
}

const NAME_MISSES_CAP: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Which party of the alliance this member sits in (0..2, or 3 for
    /// "no party"). vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp:40.
    #[serde(default)]
    pub party_no: u8,

    #[serde(default)]
    pub in_mog_house: bool,
}

const CHAT_HISTORY_CAP: usize = 256;

impl SessionState {
    /// The one append path for chat, so `chat_dropped` cannot drift from the
    /// evictions that make it true.
    pub fn push_chat(&mut self, line: ChatLine) {
        self.chat.push(line);
        if self.chat.len() > CHAT_HISTORY_CAP {
            let drop = self.chat.len() - CHAT_HISTORY_CAP;
            self.chat.drain(0..drop);
            self.chat_dropped += drop as u64;
        }
    }

    pub fn self_in_mog_house(&self) -> bool {
        if self.myroom.is_some() {
            return true;
        }
        let Some(char_id) = self.char_id else {
            return false;
        };
        self.party
            .iter()
            .find(|m| m.id == char_id)
            .map(|m| m.in_mog_house)
            .unwrap_or(false)
    }

    /// Riding is read off the broadcast animation byte, not `self_mount_id`:
    /// 0x037 carries the mount *identity* but the animation byte is what says
    /// we are on it, and it is the field every observer sees too.
    pub fn self_mounted(&self) -> bool {
        ffxi_proto::decode::animation::is_mounted(self.self_server_status)
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

    fn check_result_mut(&mut self, target_id: u32, act_index: u16) -> &mut CheckResult {
        if self.check_result.as_ref().map(|c| c.target_id) != Some(target_id) {
            self.check_result = Some(CheckResult::new(target_id, act_index));
        }
        self.check_result.as_mut().expect("just ensured Some")
    }

    /// Folds `event` into the state, returning `true` only when the state
    /// actually mutated. Paired with `watch::Sender::send_if_modified` in the
    /// session loop so the watch channel only signals real changes and
    /// downstream consumers (NativeSource scene rebuilds) skip no-op events.
    pub fn apply_event(&mut self, event: &AgentEvent) -> bool {
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
                true
            }
            AgentEvent::StageChanged { stage } => {
                let changed = self.stage != *stage || self.diagnostics.stage != Some(*stage);
                self.stage = *stage;
                self.diagnostics.stage = Some(*stage);
                changed
            }
            AgentEvent::ZoneChanged {
                to,
                myroom,
                mog_zone_flag,
                ..
            } => {
                self.zone_id = if *to == 0 { None } else { Some(*to) };

                self.myroom = *myroom;
                self.mog_zone_flag = *mog_zone_flag;
                self.sub_area = None;

                self.logout_countdown = None;
                self.death_homepoint_secs = None;
                self.death_menu_offer = None;

                self.entities.clear();
                self.party.clear();
                self.zone_generation = self.zone_generation.wrapping_add(1);

                self.current_weather = None;
                self.check_result = None;
                self.check_message = None;
                // The seller is a zone-local entity, so a bazaar cannot survive
                // the warp: LSB resolves the browsed bazaar through
                // `GetEntity(BazaarID.targid)` and drops any request once that
                // lookup fails (0x106_bazaar_buy.cpp:46-56).
                self.bazaar = None;
                // The AH counter is likewise zone-local (sendMenu from the NPC;
                // GP_CLI_COMMAND_AUC::validate gates on the zone's MISC_AH).
                self.auction = AuctionState::default();
                self.self_casting = None;
                self.self_server_status = 0;

                // Wide-scan is per-zone (server rebuilds it from the new zone's
                // entities); drop stale entries/track on any zone change or
                // home-point warp.
                self.widescan = WidescanList::default();

                // A lot/pass only holds while the player stays in the zone and
                // party; the server replays the pool on zone-in
                // (research/XiPackets/world/server/0x00D2).
                self.treasure_pool.clear();
                true
            }
            AgentEvent::SubAreaSynced { sub_area } => {
                let changed = self.sub_area != *sub_area;
                self.sub_area = *sub_area;
                changed
            }
            AgentEvent::PositionChanged { pos } => {
                let mut changed = false;
                if let Some(char_id) = self.char_id {
                    if let Some(ent) = self.entities.iter_mut().find(|e| e.id == char_id) {
                        changed = ent.pos != pos.pos
                            || ent.heading != pos.heading
                            || ent.speed != pos.speed
                            || ent.speed_base != pos.speed_base;
                        ent.pos = pos.pos;
                        ent.heading = pos.heading;
                        ent.speed = pos.speed;
                        ent.speed_base = pos.speed_base;
                    }
                }
                changed
            }
            AgentEvent::CharStatsUpdated { stats } => {
                let changed = self.char_stats != Some(*stats);
                self.char_stats = Some(*stats);
                changed
            }
            AgentEvent::EntityUpserted {
                entity,
                pos_present,
            } => {
                let latched_self_look = (self.char_id == Some(entity.id))
                    .then_some(self.self_look)
                    .flatten();
                if let Some(existing) = self.entities.iter_mut().find(|e| e.id == entity.id) {
                    let preserved_name = entity.name.clone().or_else(|| existing.name.clone());
                    let merged_kind = merge_kind(existing.kind, entity.kind);

                    let preserved_hp_pct = entity.hp_pct.or(existing.hp_pct);

                    let preserved_look = entity.look.or(existing.look).or(latched_self_look);
                    let preserved_npc_state = entity.npc_state.or(existing.npc_state);
                    let preserved_char_flags = entity.char_flags.or(existing.char_flags);
                    let preserved_mount_id = entity.mount_id.or(existing.mount_id);
                    // UPDATE_HP-gated at the source (entity_update.cpp:357/:408), so
                    // merge like char_flags — never off pos_present.
                    let preserved_name_vis = entity.name_vis.or(existing.name_vis);

                    let (
                        preserved_pos,
                        preserved_heading,
                        preserved_speed,
                        preserved_speed_base,
                        preserved_face_target,
                    ) = if *pos_present {
                        (
                            entity.pos,
                            entity.heading,
                            entity.speed,
                            entity.speed_base,
                            entity.face_target,
                        )
                    } else {
                        (
                            existing.pos,
                            existing.heading,
                            existing.speed,
                            existing.speed_base,
                            existing.face_target,
                        )
                    };
                    let merged = Entity {
                        name: preserved_name,
                        kind: merged_kind,
                        hp_pct: preserved_hp_pct,
                        look: preserved_look,
                        npc_state: preserved_npc_state,
                        char_flags: preserved_char_flags,
                        mount_id: preserved_mount_id,
                        pos: preserved_pos,
                        heading: preserved_heading,
                        speed: preserved_speed,
                        speed_base: preserved_speed_base,
                        face_target: preserved_face_target,
                        name_vis: preserved_name_vis,
                        ..entity.clone()
                    };
                    if *existing == merged {
                        false
                    } else {
                        *existing = merged;
                        true
                    }
                } else {
                    let mut inserted = entity.clone();
                    inserted.look = inserted.look.or(latched_self_look);
                    self.entities.push(inserted);
                    true
                }
            }
            AgentEvent::EntityRemoved { id } => {
                let before = self.entities.len();
                self.entities.retain(|e| e.id != *id);
                self.entities.len() != before
            }
            AgentEvent::NameExtractionMiss { miss } => {
                self.name_misses.push_back(miss.clone());
                while self.name_misses.len() > NAME_MISSES_CAP {
                    self.name_misses.pop_front();
                }
                true
            }
            AgentEvent::EntityPatched {
                id,
                act_index,
                name,
                kind,
                hp_pct,
            } => {
                let existing = self.entities.iter_mut().find(|e| {
                    id.is_some_and(|target| e.id == target)
                        || act_index.is_some_and(|target| e.act_index == target)
                });
                let mut changed = false;
                if let Some(existing) = existing {
                    if let Some(n) = name {
                        if existing.name.as_deref() != Some(n.as_str()) {
                            existing.name = Some(n.clone());
                            changed = true;
                        }
                    }
                    if let Some(k) = kind {
                        let merged = merge_kind(existing.kind, *k);
                        if existing.kind != merged {
                            existing.kind = merged;
                            changed = true;
                        }
                    }
                    if let Some(hp) = hp_pct {
                        if existing.hp_pct != Some(*hp) {
                            existing.hp_pct = Some(*hp);
                            changed = true;
                        }
                    }
                }
                changed
            }
            AgentEvent::ChatLine { line } => {
                self.push_chat(line.clone());
                true
            }
            AgentEvent::LogoutCountdown {
                seconds_remaining,
                shutdown,
            } => {
                let next = LogoutCountdown {
                    seconds_remaining: *seconds_remaining,
                    shutdown: *shutdown,
                };
                let changed = self.logout_countdown != Some(next);
                self.logout_countdown = Some(next);
                changed
            }
            AgentEvent::LogoutCountdownCancelled => {
                let changed = self.logout_countdown.is_some();
                self.logout_countdown = None;
                changed
            }
            AgentEvent::Diagnostics { diagnostics } => {
                let changed = self.diagnostics != *diagnostics;
                self.diagnostics = diagnostics.clone();
                changed
            }
            AgentEvent::NetStats { stats } => {
                let changed = self.net_stats != *stats;
                self.net_stats = *stats;
                changed
            }
            AgentEvent::SetFps { max } => {
                let changed = self.target_fps != *max;
                self.target_fps = *max;
                changed
            }
            AgentEvent::Disconnected { .. } => {
                let changed = self.stage != Stage::Disconnected
                    || self.diagnostics.stage != Some(Stage::Disconnected)
                    || self.logout_countdown.is_some()
                    || self.widescan != WidescanList::default();
                self.stage = Stage::Disconnected;
                self.diagnostics.stage = Some(Stage::Disconnected);

                self.logout_countdown = None;
                self.widescan = WidescanList::default();
                changed
            }

            AgentEvent::Error { message } => {
                self.push_chat(ChatLine {
                    spans: Vec::new(),
                    channel: ChatChannel::System,
                    sender: "<error>".into(),
                    text: message.clone(),
                    server_ts: 0,
                });
                true
            }
            AgentEvent::PartyTableReset { members } => {
                // GROUP_TBL arrived. Two shapes matter:
                //  - solo: LSB answers 0x076 with GROUP_TBL(nullptr) — Kind 0,
                //    no entries. Self is NOT in the table, and self's only
                //    source of stats is GROUP_ATTR (0x061 reply / UPDATE_HP),
                //    so wiping here would leave the frame on 0/0 until the
                //    next HP change. Self is always retained.
                //  - party: the table is the authoritative roster. Members it
                //    no longer lists are dropped; members it still lists keep
                //    their stats (the 0x0DD burst that follows refreshes them);
                //    new ids get a skeleton row.
                let self_id = self.char_id;
                let before = self.party.clone();
                self.party.retain(|m| {
                    Some(m.id) == self_id || members.iter().any(|e| e.unique_no == m.id)
                });
                for entry in members {
                    if let Some(existing) = self.party.iter_mut().find(|m| m.id == entry.unique_no)
                    {
                        existing.act_index = entry.act_index;
                        existing.zone_no = entry.zone_no;
                        existing.is_party_leader = entry.is_party_leader;
                        existing.is_alliance_leader = entry.is_alliance_leader;
                        existing.party_no = entry.party_no;
                    } else {
                        self.party.push(PartyMember {
                            id: entry.unique_no,
                            act_index: entry.act_index,
                            name: None,
                            hp: 0,
                            mp: 0,
                            tp: 0,
                            hp_pct: 0,
                            mp_pct: 0,
                            zone_no: entry.zone_no,
                            main_job: 0,
                            main_job_lv: 0,
                            sub_job: 0,
                            sub_job_lv: 0,
                            is_party_leader: entry.is_party_leader,
                            is_alliance_leader: entry.is_alliance_leader,
                            party_no: entry.party_no,
                            in_mog_house: false,
                        });
                    }
                }
                before != self.party
            }
            AgentEvent::PartyMemberUpdated { member } => {
                if let Some(existing) = self.party.iter_mut().find(|m| m.id == member.id) {
                    let preserved_name = if member.name.is_some() {
                        member.name.clone()
                    } else {
                        existing.name.clone()
                    };
                    let preserved_leader = if member.name.is_none() {
                        existing.is_party_leader
                    } else {
                        member.is_party_leader
                    };
                    let preserved_alliance = if member.name.is_none() {
                        existing.is_alliance_leader
                    } else {
                        member.is_alliance_leader
                    };
                    let preserved_party_no = if member.name.is_none() {
                        existing.party_no
                    } else {
                        member.party_no
                    };
                    let merged = PartyMember {
                        name: preserved_name,
                        is_party_leader: preserved_leader,
                        is_alliance_leader: preserved_alliance,
                        party_no: preserved_party_no,
                        ..member.clone()
                    };
                    if *existing == merged {
                        false
                    } else {
                        *existing = merged;
                        true
                    }
                } else {
                    self.party.push(member.clone());
                    true
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
                true
            }
            AgentEvent::ReactorGoalChanged { goal } => {
                let changed = self.current_goal.as_ref() != Some(goal);
                self.current_goal = Some(goal.clone());
                changed
            }
            AgentEvent::InventoryReady => {
                let changed = !self.inventory.all_loaded;
                self.inventory.all_loaded = true;
                changed
            }

            AgentEvent::ForcedMove { target, .. } => {
                let mut changed = false;
                if let Some(char_id) = self.char_id {
                    if let Some(ent) = self.entities.iter_mut().find(|e| e.id == char_id) {
                        changed = ent.pos != target.pos || ent.heading != target.heading;
                        ent.pos = target.pos;
                        ent.heading = target.heading;
                    }
                }
                changed
            }
            AgentEvent::LowHp { .. }
            | AgentEvent::PartyMemberLowHp { .. }
            | AgentEvent::EngagedBy { .. }
            | AgentEvent::TellReceived { .. }
            | AgentEvent::SceneSummary { .. }
            | AgentEvent::ActionStarted { .. }
            | AgentEvent::EntityEmoted { .. }
            | AgentEvent::HumanInControl { .. }
            | AgentEvent::HumanReleased
            | AgentEvent::MusicChanged { .. }
            | AgentEvent::MusicVolumeChanged { .. }
            | AgentEvent::LevelUp { .. }
            | AgentEvent::SkillLevelUp { .. }
            | AgentEvent::VanaTimeSynced { .. } => false,
            AgentEvent::InventoryUpdated { container, update } => {
                let entry = self.inventory.containers.entry(*container).or_default();
                match update {
                    InventoryUpdate::Capacities { capacities } => {
                        // Zeros apply too: 0 is LSB's "container disabled"
                        // sentinel (e.g. an expired Mog Locker lease across a
                        // zone change) — sticky grants would keep offering a
                        // bag the server rejects (s2c/0x01c_item_max.cpp:52).
                        for (id, cap) in capacities.iter().enumerate() {
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
                    }
                }
                true
            }
            AgentEvent::DeliveryBoxUpdated { box_no, update } => {
                let dbox = &mut self.delivery_box;
                match update {
                    DeliveryBoxUpdate::Opened => {
                        *dbox = DeliveryBoxState {
                            open: Some(*box_no),
                            slots: vec![None; ffxi_proto::map::pbx::SLOT_COUNT],
                            queued: 0,
                            recipient: None,
                            recipient_status: RecipientStatus::Unset,
                        };
                    }
                    DeliveryBoxUpdate::Closed => {
                        *dbox = DeliveryBoxState::default();
                    }
                    DeliveryBoxUpdate::SlotChanged { slot, item } => {
                        if dbox.slots.len() < ffxi_proto::map::pbx::SLOT_COUNT {
                            dbox.slots.resize(ffxi_proto::map::pbx::SLOT_COUNT, None);
                        }
                        if let Some(cell) = dbox.slots.get_mut(*slot as usize) {
                            *cell = item.clone();
                        }
                    }
                    DeliveryBoxUpdate::PendingCount { count } => {
                        dbox.queued = *count;
                    }
                    DeliveryBoxUpdate::RecipientPending { name } => {
                        dbox.recipient = Some(name.clone());
                        dbox.recipient_status = RecipientStatus::Pending;
                    }
                    DeliveryBoxUpdate::RecipientCheck { ok, same_account } => {
                        if *ok {
                            dbox.recipient_status = RecipientStatus::Ok {
                                same_account: *same_account,
                            };
                        } else {
                            dbox.recipient = None;
                            dbox.recipient_status = RecipientStatus::NoSuchChar;
                        }
                    }
                    DeliveryBoxUpdate::Failed { .. } => return false,
                }
                true
            }
            AgentEvent::EquipUpdated {
                slot,
                container,
                container_index,
            } => {
                let mut changed = false;
                if let Some(cell) = self.equipment.get_mut(*slot as usize) {
                    // The server reports an empty/unequipped slot as inventory
                    // index 0 (charutils.cpp:2268 queueEquipChange(LOC_INVENTORY,
                    // 0, ...)). Index 0 is reserved (Gil in LOC_INVENTORY) and is
                    // never a real equipped item, so treat it as cleared — else
                    // resolve_equipment joins it to Gil.
                    let next = (*container_index != 0).then_some(EquippedRef {
                        container: *container,
                        container_index: *container_index,
                    });
                    changed = *cell != next;
                    *cell = next;
                }
                changed
            }
            AgentEvent::EquipCleared => {
                let changed = self.equipment.iter().any(|c| c.is_some());
                self.equipment = [None; EQUIPMENT_SLOTS];
                changed
            }
            AgentEvent::SelfLookUpdated { look } => {
                let mut changed = self.self_look != Some(*look);
                self.self_look = Some(*look);
                if let Some(char_id) = self.char_id {
                    if let Some(ent) = self.entities.iter_mut().find(|e| e.id == char_id) {
                        changed |= ent.look != Some(*look);
                        ent.look = Some(*look);
                    }
                }
                changed
            }
            AgentEvent::SpellsKnownUpdated { ids } => {
                let changed = self.spells_known != *ids;
                self.spells_known = ids.clone();
                changed
            }
            AgentEvent::CommandDataUpdated {
                weapon_skills,
                job_abilities,
                pet_abilities,
            } => {
                let changed = self.weaponskills_known != *weapon_skills
                    || self.job_abilities_known != *job_abilities
                    || self.pet_abilities_known != *pet_abilities;
                self.weaponskills_known = weapon_skills.clone();
                self.job_abilities_known = job_abilities.clone();
                self.pet_abilities_known = pet_abilities.clone();
                changed
            }
            AgentEvent::KeyItemsUpdated {
                table_index,
                ids,
                seen_ids,
            } => {
                let base = *table_index as usize * KEY_ITEMS_PER_TABLE;
                let table_range = base..base + KEY_ITEMS_PER_TABLE;
                let replace_table = |list: &mut Vec<u16>, incoming: &[u16]| {
                    let before = list.clone();
                    list.retain(|id| !table_range.contains(&(*id as usize)));
                    list.extend(incoming.iter().copied());
                    list.sort_unstable();
                    list.dedup();
                    *list != before
                };
                let owned_changed = replace_table(&mut self.key_items, ids);
                let seen_changed = replace_table(&mut self.key_items_seen, seen_ids);
                owned_changed || seen_changed
            }

            AgentEvent::CheckEquipReceived {
                target_id,
                act_index,
                items,
            } => {
                let r = self.check_result_mut(*target_id, *act_index);
                let mut changed = false;
                for &(slot, item_no) in items {
                    if let Some(cell) = r.equipped.get_mut(slot as usize) {
                        changed |= *cell != Some(item_no);
                        *cell = Some(item_no);
                    }
                }
                changed
            }
            AgentEvent::CheckGeneralReceived {
                target_id,
                act_index,
                main_job,
                sub_job,
                main_job_lv,
                sub_job_lv,
                master_lv,
                linkshell,
            } => {
                let r = self.check_result_mut(*target_id, *act_index);
                let next = (*main_job, *sub_job, *main_job_lv, *sub_job_lv, *master_lv);
                let mut changed = (
                    r.main_job,
                    r.sub_job,
                    r.main_job_lv,
                    r.sub_job_lv,
                    r.master_lv,
                ) != next;
                (
                    r.main_job,
                    r.sub_job,
                    r.main_job_lv,
                    r.sub_job_lv,
                    r.master_lv,
                ) = next;
                changed |= r.linkshell != *linkshell;
                r.linkshell.clone_from(linkshell);
                changed
            }
            AgentEvent::CheckMessageReceived { name, message } => {
                let next = CheckMessage {
                    name: name.clone(),
                    message: message.clone(),
                };
                let changed = self.check_message.as_ref() != Some(&next);
                self.check_message = Some(next);
                changed
            }
            AgentEvent::CheckCleared => {
                let changed = self.check_result.is_some() || self.check_message.is_some();
                self.check_result = None;
                self.check_message = None;
                changed
            }
            AgentEvent::BazaarOpened {
                seller_id,
                seller_index,
                seller_name,
            } => {
                self.bazaar = Some(BazaarView {
                    seller_id: *seller_id,
                    seller_index: *seller_index,
                    seller_name: seller_name.clone(),
                    items: Vec::new(),
                });
                true
            }
            AgentEvent::BazaarItemReceived {
                index,
                item_no,
                quantity,
                price,
                tax_rate,
            } => {
                let Some(view) = self.bazaar.as_mut() else {
                    return false;
                };
                let before = view.items.clone();
                view.items.retain(|it| it.index != *index);
                if *price != 0 && *quantity != 0 {
                    view.items.push(BazaarItem {
                        index: *index,
                        item_no: *item_no,
                        quantity: *quantity,
                        price: *price,
                        tax_rate: *tax_rate,
                    });
                    view.items.sort_unstable_by_key(|it| it.index);
                }
                view.items != before
            }
            AgentEvent::BazaarClosed => {
                let changed = self.bazaar.is_some();
                self.bazaar = None;
                changed
            }
            AgentEvent::BazaarBuyResult { ok } => {
                let text = if *ok {
                    "Purchased.".to_string()
                } else {
                    "The purchase failed.".to_string()
                };
                self.push_chat(ChatLine {
                    spans: Vec::new(),
                    channel: ChatChannel::System,
                    sender: "<bazaar>".into(),
                    text,
                    server_ts: 0,
                });
                true
            }
            AgentEvent::BazaarSoldToOther {
                buyer,
                index,
                quantity,
            } => {
                let item = self
                    .bazaar
                    .as_ref()
                    .and_then(|v| v.items.iter().find(|it| it.index == *index))
                    .and_then(|it| ffxi_vocab::item_names::lookup(it.item_no))
                    .unwrap_or("an item");
                self.push_chat(ChatLine {
                    spans: Vec::new(),
                    channel: ChatChannel::System,
                    sender: "<bazaar>".into(),
                    text: format!("{buyer} purchased {quantity}x {item}."),
                    server_ts: 0,
                });
                true
            }
            AgentEvent::EventStart { .. }
            | AgentEvent::KeyRotated { .. }
            | AgentEvent::CutsceneStarted { .. }
            | AgentEvent::CutsceneCue { .. }
            | AgentEvent::CutsceneEnded => false,
            AgentEvent::EventDialog { dialog } => {
                let changed = self.dialog.as_ref() != Some(dialog);
                self.dialog = Some(dialog.clone());
                changed
            }
            AgentEvent::ShopUpdated { shop } => {
                let changed = self.shop.as_ref() != Some(shop);
                self.shop = Some(shop.clone());
                changed
            }
            AgentEvent::ShopSellAppraisal {
                price,
                item_index,
                count,
            } => {
                self.push_chat(ChatLine {
                    spans: Vec::new(),
                    channel: ChatChannel::System,
                    sender: "<shop>".into(),
                    text: format!(
                        "Appraisal: slot {item_index} x{count} sells for {price} gil each \
                         — `/sell confirm` to accept"
                    ),
                    server_ts: 0,
                });
                true
            }
            AgentEvent::StatusIconsUpdated { icons, expiries } => {
                let changed = self.status_icons != *icons || self.status_icon_expiries != *expiries;
                self.status_icons = icons.clone();
                self.status_icon_expiries = expiries.clone();
                changed
            }
            AgentEvent::AbilityRecastsUpdated { recasts } => {
                let changed = self.ability_recasts != *recasts;
                self.ability_recasts = recasts.clone();
                changed
            }
            AgentEvent::JobInfoUpdated { info } => {
                let changed = self.job_info != Some(*info);
                self.job_info = Some(*info);
                changed
            }
            AgentEvent::MogHouse2fUnlockUpdated { unlocked } => {
                let changed = self.mh_2f_unlocked != Some(*unlocked);
                self.mh_2f_unlocked = Some(*unlocked);
                changed
            }
            AgentEvent::TreasurePoolUpdated { slot } => {
                self.treasure_pool
                    .resize(ffxi_proto::decode::TREASURE_POOL_SIZE, None);
                match self.treasure_pool.get_mut(slot.slot as usize) {
                    Some(dest) => {
                        let changed = dest.as_ref() != Some(slot.as_ref());
                        *dest = Some((**slot).clone());
                        changed
                    }
                    None => false,
                }
            }
            AgentEvent::TreasurePoolCleared { slot } => {
                match self.treasure_pool.get_mut(*slot as usize) {
                    Some(dest) => dest.take().is_some(),
                    None => false,
                }
            }
            AgentEvent::DeathTimerUpdated {
                seconds_until_homepoint,
            } => {
                let changed = self.death_homepoint_secs != *seconds_until_homepoint
                    || (seconds_until_homepoint.is_none() && self.death_menu_offer.is_some());
                self.death_homepoint_secs = *seconds_until_homepoint;
                if seconds_until_homepoint.is_none() {
                    self.death_menu_offer = None;
                }
                changed
            }
            AgentEvent::DeathMenuUpdated { offer } => {
                let changed = self.death_menu_offer != *offer;
                self.death_menu_offer = *offer;
                changed
            }
            AgentEvent::WeatherUpdated { weather_number } => {
                let changed = self.current_weather != Some(*weather_number);
                self.current_weather = Some(*weather_number);
                changed
            }
            AgentEvent::EventEnded => {
                let changed = self.dialog.is_some() || self.shop.is_some();
                self.dialog = None;

                self.shop = None;
                changed
            }
            AgentEvent::SelfServerStatus { status, mount_id } => {
                let changed = self.self_server_status != *status || self.self_mount_id != *mount_id;
                self.self_server_status = *status;
                self.self_mount_id = *mount_id;
                changed
            }
            // Machine inputs (consumed by the reactor, not the rendered projection).
            AgentEvent::FishingCast { .. }
            | AgentEvent::FishingServerPhase { .. }
            | AgentEvent::FishingEnded => false,
            // Only labels a cast already in flight. The hook message is a plain
            // zone-dialog line whose index another message in the same zone
            // could collide with, so it must never be what *starts* the HUD —
            // the server's FISHING_START phase does that first.
            AgentEvent::FishHookedSize { size } => match self.self_fishing.as_mut() {
                Some(f) => {
                    let changed = f.size != Some(*size);
                    f.size = Some(*size);
                    changed
                }
                None => false,
            },
            AgentEvent::FishHooked { params } => {
                let f = self.self_fishing.get_or_insert(SelfFishing::starting(1));
                let changed = f.fish != Some(*params) || f.fish_hp != params.stamina;
                f.fish = Some(*params);
                f.fish_hp = params.stamina;
                changed
            }
            AgentEvent::FishingPhaseChanged { phase } => match phase {
                Some(p) => {
                    let changed = self.self_fishing.map(|f| f.phase) != Some(*p);
                    self.self_fishing
                        .get_or_insert(SelfFishing::starting(*p))
                        .phase = *p;
                    changed
                }
                None => {
                    let changed = self.self_fishing.is_some();
                    self.self_fishing = None;
                    changed
                }
            },
            AgentEvent::EmoteListUpdated {
                job_bits,
                chair_bits,
            } => {
                let changed =
                    self.emote_jobs != Some(*job_bits) || self.emote_chairs != Some(*chair_bits);
                self.emote_jobs = Some(*job_bits);
                self.emote_chairs = Some(*chair_bits);
                changed
            }
            AgentEvent::FishingProgress { fish_hp, arrow } => {
                let mut changed = false;
                if let Some(f) = self.self_fishing.as_mut() {
                    changed = f.fish_hp != *fish_hp || f.arrow != *arrow;
                    f.fish_hp = *fish_hp;
                    f.arrow = *arrow;
                }
                changed
            }
            AgentEvent::WidescanListStart => {
                let changed = !self.widescan.entries.is_empty() || !self.widescan.building;
                self.widescan.entries.clear();
                self.widescan.building = true;
                changed
            }
            AgentEvent::WidescanEntryReceived { entry } => {
                if !self.widescan.building {
                    return false;
                }
                self.widescan.entries.push(entry.clone());
                true
            }
            AgentEvent::WidescanListEnd => {
                let changed = self.widescan.building;
                self.widescan.building = false;
                changed
            }
            AgentEvent::WidescanTrackUpdated { tracked } => {
                let changed = self.widescan.tracked != *tracked;
                self.widescan.tracked = *tracked;
                changed
            }
            AgentEvent::AuctionMenuOpened => {
                let changed = !self.auction.open || self.auction.busy.is_some();
                self.auction.open = true;
                self.auction.busy = None;
                changed
            }
            AgentEvent::AuctionOpStarted { op } => {
                let changed = self.auction.busy != Some(*op);
                self.auction.busy = Some(*op);
                changed
            }
            AgentEvent::AuctionBrowseResults {
                category,
                total,
                listings,
            } => {
                self.auction.browse = Some(AhCatalogView {
                    category: *category,
                    total: *total,
                    listings: listings.clone(),
                });
                self.auction.busy = None;
                true
            }
            AgentEvent::AuctionHistoryResults { history } => {
                self.auction.history = Some(history.clone());
                self.auction.busy = None;
                true
            }
            AgentEvent::AuctionSearchFailed { .. } => {
                let changed = self.auction.busy.is_some();
                self.auction.busy = None;
                changed
            }
            AgentEvent::AuctionSellQuote { quote, .. } => {
                let changed = self.auction.fee_quote != *quote;
                self.auction.fee_quote = *quote;
                changed
            }
            AgentEvent::AuctionSellResult { ok, .. } => {
                let changed = *ok && self.auction.fee_quote.is_some();
                if *ok {
                    self.auction.fee_quote = None;
                }
                changed
            }
            AgentEvent::AuctionBidResult { .. } => {
                let changed = self.auction.busy.is_some();
                self.auction.busy = None;
                changed
            }
            AgentEvent::AuctionSalesStatusReset { result } => {
                if *result != ffxi_proto::decode::AUCTION_RESULT_OPEN {
                    return false;
                }
                let changed = self.auction.sales_status.iter().any(Option::is_some);
                self.auction.sales_status = Default::default();
                changed
            }
            AgentEvent::AuctionSalesSlot { slot, sale } => {
                let Some(cell) = self.auction.sales_status.get_mut(*slot as usize) else {
                    return false;
                };
                let changed = cell != sale;
                *cell = sale.clone();
                changed
            }
            AgentEvent::AuctionCancelResult { .. } => false,
            AgentEvent::SelfCastStarted { name, total_ms } => {
                self.self_casting = Some(SelfCasting {
                    name: name.clone(),
                    elapsed_ms: 0,
                    total_ms: *total_ms,
                    interrupted: false,
                });
                true
            }
            AgentEvent::SelfCastProgress { elapsed_ms } => {
                let mut changed = false;
                if let Some(c) = self.self_casting.as_mut() {
                    changed = c.elapsed_ms != *elapsed_ms;
                    c.elapsed_ms = *elapsed_ms;
                }
                changed
            }
            AgentEvent::SelfCastEnded { interrupted } => {
                let changed = self.self_casting.is_some();
                if *interrupted {
                    if let Some(c) = self.self_casting.as_mut() {
                        c.interrupted = true;
                    }
                }
                self.self_casting = None;
                changed
            }
        }
    }
}

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

        #[serde(default)]
        myroom: Option<MyRoomInfo>,

        #[serde(default)]
        mog_zone_flag: bool,
    },
    /// `SubMapNumber` out of 0x00A LOGIN, emitted right after the
    /// [`AgentEvent::ZoneChanged`] that clears it.
    SubAreaSynced {
        sub_area: Option<u16>,
    },
    PositionChanged {
        pos: Position,
    },
    CharStatsUpdated {
        stats: CharStatsRaw,
    },
    EntityUpserted {
        entity: Entity,

        #[serde(default = "default_true")]
        pos_present: bool,
    },
    EntityRemoved {
        id: u32,
    },

    NameExtractionMiss {
        miss: NameExtractionMiss,
    },

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

    EventDialog {
        dialog: DialogState,
    },

    /// An event session opened. Distinct from [`AgentEvent::EventStart`],
    /// which also fires for client-local menus that run no script.
    CutsceneStarted {
        event_id: u32,
    },

    /// One staging cue the running event script emitted.
    CutsceneCue {
        cue: CutsceneCue,
    },

    /// The event session closed and everything scoped to it reverts. Sent on
    /// every exit — script end, cancel, watchdog release, zone change,
    /// disconnect — because an event body routinely locks the camera and never
    /// unlocks it (retail's teardown is the zone change).
    CutsceneEnded,

    ShopUpdated {
        shop: ShopState,
    },

    /// Server appraisal answer to a SHOP_SELL_REQ (s2c 0x03D): `price` is per unit.
    ShopSellAppraisal {
        price: u32,
        item_index: u8,
        count: u32,
    },

    StatusIconsUpdated {
        icons: Vec<u16>,
        #[serde(default)]
        expiries: Vec<u32>,
    },

    AbilityRecastsUpdated {
        recasts: Vec<(u16, u32)>,
    },

    JobInfoUpdated {
        info: JobInfoState,
    },

    MogHouse2fUnlockUpdated {
        unlocked: bool,
    },

    /// A treasure-pool slot was filled or its lot state changed (s2c 0x0D2, and
    /// the non-final 0x0D3 verdicts).
    TreasurePoolUpdated {
        slot: Box<TreasurePoolSlot>,
    },
    /// A pool slot emptied: won, lost, or expired (s2c 0x0D3).
    TreasurePoolCleared {
        slot: u8,
    },

    WeatherUpdated {
        weather_number: u16,
    },

    VanaTimeSynced {
        game_time: u32,
    },

    LogoutCountdown {
        seconds_remaining: u16,

        shutdown: bool,
    },
    /// Stand-up cancels leavegame server-side (`MakeEntityStandUp` drops the
    /// HEALING effect, `healing.onEffectLose` removes LEAVEGAME) without any
    /// 0x053 cancel packet — the client sees it as its own CHAR_PC status
    /// flipping off HEALING and clears the countdown here.
    LogoutCountdownCancelled,
    EventEnded,

    ActionStarted {
        actor_id: u32,
        action_id: u32,
        action_kind: u8,
        target_id: Option<u32>,
        result: Option<ffxi_proto::melee::MeleeResult>,
        animation: Option<u16>,
    },

    /// The self player began casting a spell (optimistic, on send). Drives the
    /// Enhanced cast bar. Only emitted for spells with a non-zero cast time.
    SelfCastStarted {
        name: String,
        total_ms: u32,
    },

    /// Per-tick cast-bar progress while a spell is in flight.
    SelfCastProgress {
        elapsed_ms: u32,
    },

    /// The self cast resolved (`interrupted` = false) or was interrupted
    /// (moved/paralyzed/rejected). Clears the cast bar.
    SelfCastEnded {
        interrupted: bool,
    },

    /// s2c 0x05A MOTIONMES: an entity performed an emote. `emote_id` is the
    /// wire MesNum (job emotes arrive rebased to 74..=95), `mode` the EmoteMode
    /// byte, `target_id` 0 when untargeted.
    EntityEmoted {
        actor_id: u32,
        actor_index: u16,
        target_id: u32,
        target_index: u16,
        emote_id: u16,
        param: u16,
        mode: u8,
    },

    /// s2c 0x11A EMOTE_LIST: job-emote/chair unlock bitfields.
    EmoteListUpdated {
        job_bits: u32,
        chair_bits: u16,
    },
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

    NetStats {
        stats: NetStats,
    },

    PartyMemberUpdated {
        member: PartyMember,
    },

    /// GROUP_TBL (s2c 0x0C8) arrived: the server is sending a fresh party
    /// definition. Clear the party list and seed it with the skeleton entries
    /// from the table; the full stats follow in GROUP_LIST (0x0DD) packets.
    PartyTableReset {
        members: Vec<ffxi_proto::decode::GroupTblEntry>,
    },

    LowHp {
        pct: u8,
    },

    PartyMemberLowHp {
        id: u32,
        pct: u8,
    },

    EngagedBy {
        entity_id: u32,
    },

    ForcedMove {
        mode: u8,
        target: Position,

        duration_ms: u32,
    },

    SetFps {
        max: u32,
    },

    TellReceived {
        from: String,
        text: String,
    },

    Reconnected {
        downtime_ms: u64,
    },

    SceneSummary {
        text: String,
    },

    InventoryUpdated {
        container: u8,
        update: InventoryUpdate,
    },

    InventoryReady,

    DeliveryBoxUpdated {
        box_no: DeliveryBoxNo,
        update: DeliveryBoxUpdate,
    },

    EquipUpdated {
        slot: u8,
        container: u8,
        container_index: u8,
    },

    EquipCleared,

    /// Self's appearance from s2c 0x051 GRAP_LIST — the only push channel for
    /// it, since LSB never broadcasts a 0x00D CHAR_PC about a player to that
    /// player (vendor/server/src/map/zone_entities.cpp
    /// `CZoneEntities::UpdateEntityPacket`).
    SelfLookUpdated {
        look: ffxi_proto::decode::LookData,
    },

    SpellsKnownUpdated {
        ids: Vec<u16>,
    },

    CommandDataUpdated {
        weapon_skills: Vec<u16>,
        job_abilities: Vec<u16>,
        pet_abilities: Vec<u16>,
    },

    KeyItemsUpdated {
        table_index: u16,
        ids: Vec<u16>,
        #[serde(default)]
        seen_ids: Vec<u16>,
    },

    ReactorGoalChanged {
        goal: ReactorGoalSnapshot,
    },

    HumanInControl {
        reason: String,
    },

    HumanReleased,

    MusicChanged {
        slot: u8,
        track_id: u16,
    },

    DeathTimerUpdated {
        seconds_until_homepoint: Option<u32>,
    },

    /// s2c 0x0F9 `GP_SERV_COMMAND_RES`: `None` restores the default
    /// home-point-only menu; `Some` offers Raise/Reraise or Tractor.
    DeathMenuUpdated {
        offer: Option<ffxi_proto::decode::DeathMenuOffer>,
    },

    MusicVolumeChanged {
        slot: u8,
        volume: u8,
    },

    LevelUp {
        player_id: u32,
    },

    SkillLevelUp {
        skill_id: u16,
        level: u32,
    },

    /// Self has cast a line: the server set FISHING_START with this hook delay (frames).
    /// Decoded from 0x037 GP_SERV_SERVERSTATUS.
    FishingCast {
        hook_delay: u8,
    },

    /// A fish bit; the mini-game can begin. Decoded from 0x115 GP_SERV_COMMAND_FISH.
    FishHooked {
        params: FishParams,
    },

    /// The hooked-fish size, from the 0x036 TALKNUM the server pushes just
    /// ahead of 0x115. Only the mini-game bar's label depends on it.
    FishHookedSize {
        size: FishSize,
    },

    /// Raw self animation phase straight from the 0x037 byte (machine input for the
    /// resolution/release handshake). Distinct from `FishingPhaseChanged`, which is the
    /// reactor machine's published view.
    FishingServerPhase {
        phase: Option<u8>,
    },

    /// The whole 0x037 animation byte for self (`ANIMATION_*` in
    /// vendor/server/src/map/entities/baseentity.h). The server owns this — it
    /// starts and ends resting on its own (damage, status effects) — so the
    /// renderer reconciles its optimistic local stance against it.
    SelfServerStatus {
        status: u8,
        /// 0x037's `mount_id` — which mount, not whether one is being ridden.
        /// It rides this event because both fall out of the same packet and the
        /// pair is only meaningful read together.
        mount_id: u8,
    },

    /// The reactor fishing machine's view phase (0..=6, see `ffxi_actor`'s `fishing_clip`),
    /// or `None` once fishing ends. This is what drives the self pose / HUD visibility.
    FishingPhaseChanged {
        phase: Option<u8>,
    },

    /// Mini-game HUD progress published by the reactor's fishing machine each tick.
    FishingProgress {
        fish_hp: u16,
        arrow: Option<FishingArrow>,
    },

    /// The server released the fishing lock (0x052 EVENTUCOFF mode Fishing): a rejected
    /// cast (no rod/bait/spot) or the end of fishing. Machine input that aborts to idle.
    FishingEnded,

    /// One s2c 0x0C9 EQUIP_INSPECT EQUIPMENT batch (OptionFlag 0x03): up to 8
    /// `(slot, item_no)` pairs of the checked PC's gear; slot ids follow
    /// SAVE_EQUIP_KIND (0 = Main .. 15 = Back).
    CheckEquipReceived {
        target_id: u32,
        act_index: u16,
        items: Vec<(u8, u16)>,
    },

    /// s2c 0x0C9 EQUIP_INSPECT GENERAL (OptionFlag 0x01): the checked PC's jobs
    /// and levels (zeroed while the target is /anon) plus their linkshell.
    CheckGeneralReceived {
        target_id: u32,
        act_index: u16,
        main_job: u8,
        sub_job: u8,
        main_job_lv: u8,
        sub_job_lv: u8,
        master_lv: u8,
        /// Already unpacked from the 6-bit `sComLinkName`; empty = no pearl.
        linkshell: String,
    },

    /// s2c 0x0CA INSPECT_MESSAGE: the checked PC's name and bazaar message.
    CheckMessageReceived {
        name: String,
        message: String,
    },

    /// Outbound /check dispatched: drop the previous target's accumulated result.
    CheckCleared,

    /// s2c 0x105 BAZAAR_LIST: one priced row of the browsed bazaar, merged by
    /// `index`. A row that is no longer for sale (price or quantity zero)
    /// removes it.
    BazaarItemReceived {
        index: u8,
        item_no: u16,
        quantity: u32,
        price: u32,
        tax_rate: u16,
    },

    /// Our c2s 0x105 was dispatched: start a fresh (empty) bazaar view for the
    /// seller so the rows have somewhere to land.
    BazaarOpened {
        seller_id: u32,
        seller_index: u16,
        seller_name: String,
    },

    /// s2c 0x107, or our own exit: the browsed bazaar is gone.
    BazaarClosed,

    /// s2c 0x106: result of our purchase attempt.
    BazaarBuyResult {
        ok: bool,
    },

    /// s2c 0x109: another customer bought `quantity` of row `index` while we
    /// browse; a refreshed 0x105 row follows.
    BazaarSoldToOther {
        buyer: String,
        index: u8,
        quantity: u32,
    },

    /// s2c 0x0F6 ListStart: a fresh wide-scan list is about to arrive — clear the
    /// accumulator and mark it building.
    WidescanListStart,

    /// s2c 0x0F4: one wide-scan entry, appended while the list is building.
    WidescanEntryReceived {
        entry: WidescanEntry,
    },

    /// s2c 0x0F6 ListEnd: the wide-scan list is complete.
    WidescanListEnd,

    /// s2c 0x0F5: the tracked entity moved (`Some`) or was lost/`State == Lose`
    /// (`None`, which clears the tracked marker).
    WidescanTrackUpdated {
        tracked: Option<WidescanPos>,
    },

    /// s2c 0x04C Open: the AH counter menu opened (lua_baseentity sendMenu).
    AuctionMenuOpened,

    /// An AH op with a spinner was dispatched; cleared by its result event.
    AuctionOpStarted {
        op: AuctionBusy,
    },

    /// Search-server TCP_AH_REQUEST answered (all pages merged).
    AuctionBrowseResults {
        category: u8,
        total: u16,
        listings: Vec<AhListingView>,
    },

    /// Search-server TCP_AH_HISTORY_SINGLE/_STACK answered.
    AuctionHistoryResults {
        history: AhHistoryView,
    },

    /// A search-server round-trip failed (unreachable, timeout, bad frame).
    AuctionSearchFailed {
        message: String,
    },

    /// s2c AskCommit: the listing-fee quote (`quote: None` on rejection, with
    /// the LSB message code in `result` — 197 from auctionutils SellingItems).
    AuctionSellQuote {
        quote: Option<AhFeeQuote>,
        result: u8,
    },

    /// s2c LotIn: `ok` on Result 1 ("Merchandise put up on auction"), else the
    /// raw LSB message code in `result`.
    AuctionSellResult {
        ok: bool,
        result: u8,
    },

    /// s2c Bid echo. `ok` on Result 1; 0xC5 = outbid/none cheap enough ("You
    /// were unable to buy the X for N gil."), 0xE5 = no space / Rare dupe
    /// (auctionutils PurchasingItems). `quantity` echoes ItemStacks: the stack
    /// size for a stack bid, 1 for a single.
    AuctionBidResult {
        ok: bool,
        item_no: u16,
        price: u32,
        quantity: u32,
        result: u8,
    },

    /// s2c Info ack: Result 1 restarts the 7-slot sales-status stream
    /// (auctionutils OpenListOfSales clears its history first); 246 = throttled.
    AuctionSalesStatusReset {
        result: u8,
    },

    /// One sales-status slot (s2c 0x0C/LotCheck row); `sale: None` empties it.
    AuctionSalesSlot {
        slot: u8,
        sale: Option<AhSaleStatus>,
    },

    /// s2c LotCancel verdict: Result 0 = returned to inventory, 0xE5 = failed
    /// (inventory full; the slot keeps its row).
    AuctionCancelResult {
        slot: u8,
        ok: bool,
        result: u8,
    },
}

pub const GROUND_CORRECTION_XY_EPSILON_YALMS: f32 = 0.05;

pub fn ground_correction_matches(
    expected_x: f32,
    expected_y: f32,
    actual_x: f32,
    actual_y: f32,
) -> bool {
    let dx = expected_x - actual_x;
    let dy = expected_y - actual_y;
    dx * dx + dy * dy <= GROUND_CORRECTION_XY_EPSILON_YALMS * GROUND_CORRECTION_XY_EPSILON_YALMS
}

pub fn apply_ground_height_correction(
    position: &mut Position,
    expected_x: f32,
    expected_y: f32,
    corrected_z: f32,
) -> bool {
    if !ground_correction_matches(expected_x, expected_y, position.pos.x, position.pos.y) {
        return false;
    }
    position.pos.z = corrected_z;
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum AgentCommand {
    Move {
        x: f32,
        y: f32,
        z: f32,
        heading: u8,
    },

    StopMove,

    /// Client-side height repair for the under-every-floor wedge (kuluu-mo4q).
    /// The identity and horizontal coordinates identify the position whose
    /// height was diagnosed; the session applies `z` only while they still
    /// match.
    GroundCorrection {
        #[serde(default)]
        zone_id: u16,
        #[serde(default)]
        self_id: u32,
        x: f32,
        y: f32,
        z: f32,
        heading: u8,
    },

    /// Free-text answer to a client-local text-entry dialog frame (currently
    /// only the delivery-box recipient prompt). Ignored when no text entry is
    /// pending.
    TextInput {
        text: String,
    },

    RequestZoneChange {
        line_id: u32,
    },

    MogHouseExit {
        kind: MogHouseExit,
    },

    /// c2s 0x100 MYROOM_JOB; `None` → 0 = keep current. LSB acts only on
    /// indices > 0, so there is deliberately no remove-subjob form
    /// (vendor/server/src/map/packets/c2s/0x100_myroom_job.cpp).
    ChangeJob {
        main_job: Option<u8>,
        sub_job: Option<u8>,
    },

    /// Client-local open of the same menu s2c 0x02E OPENMOGMENU triggers
    /// (vendor/server/src/map/packets/s2c/0x02e_openmogmenu.h).
    OpenMogMenu,

    /// Cast lots on a treasure-pool slot — c2s 0x041 TROPHY_ENTRY. The server
    /// rolls the value, so there is nothing to send but the slot.
    TreasureLot {
        slot: u8,
    },

    /// Pass on a treasure-pool slot — c2s 0x042 TROPHY_ABSENCE.
    TreasurePass {
        slot: u8,
    },

    /// Mark key items seen — c2s 0x064 GP_CLI_COMMAND_SCENARIOITEM with the
    /// table's full updated LookItemFlag bitset
    /// (vendor/server/src/map/packets/c2s/0x064_scenarioitem.cpp ORs each set bit).
    MarkKeyItemsSeen {
        table_index: u16,
        ids: Vec<u16>,
    },

    /// c2s 0x0F1 GP_CLI_COMMAND_BUFFCANCEL — click a status effect off by its
    /// icon id. The server deletes every effect with that icon
    /// (DelStatusEffectsByIcon) and does NOT re-check cancelability, so the
    /// caller must gate on `ffxi_vocab::status_effects::is_cancelable`
    /// (vendor/server/src/map/packets/c2s/0x0f1_buffcancel.cpp).
    CancelBuff {
        icon: u16,
    },

    /// c2s 0x0F2 GP_CLI_COMMAND_SUBMAPCHANGE — report a client-side sub-area
    /// latch change (the geometry-driven boundary crossing lives in
    /// `ffxi-dat`/`kuluu-render`; this is only the wire report). Sent with
    /// State = General (`ffxi_proto::map::submap::state::GENERAL`); Event
    /// requires an active server event and is not used here.
    /// `sub_area` is the LSB `SubMapNumber`/`PChar->loc.boundary` value —
    /// pass `ffxi_proto::map::submap::NO_SUB_AREA` (0) for "no sub-area",
    /// matching retail's clamp of a negative boundary.
    ReportSubArea {
        sub_area: u16,
    },

    EndEvent,

    EndEventChoice {
        event_id: u32,
        act_index: u16,
        event_num: u16,
        choice: u32,
    },

    /// Answer a server customMenu (GMPROMPT/`_CUSTOM_MENU`). `option = Some(label)`
    /// picks that row; `None` cancels. The session builds the `_CUSTOM_MENU` tell
    /// the server's HandleCustomMenu parser expects (vendor/server/src/map/packets/
    /// c2s/0x0b6_chat_name.cpp, luautils.cpp HandleCustomMenu).
    CustomMenuRespond {
        title: String,
        option: Option<String>,
    },

    Disconnect,

    ReqLogout {
        kind: ReqLogoutKind,
    },

    Snapshot,

    /// Focus-less GUI driving (kuluu-0pof): hold a simulated movement input
    /// (`forward`/`strafe` in {-1,0,1}) for `duration_ms`, fed through the same
    /// `input.rs` path as WASD. Intercepted by the socket decoder, never
    /// forwarded to the session.
    DebugDrive {
        forward: i32,
        strafe: i32,
        duration_ms: u64,
    },

    /// Focus-less GUI driving (kuluu-0pof): trigger the `/debug heights` grounding
    /// dump; the numbers are logged (`target: "debug_heights"`) so they are
    /// readable without a screenshot. Intercepted by the socket decoder.
    DebugHeights,

    /// Capture the rendered frame to `path` (default `screenshot-<n>.png`).
    /// Reads back the render target on the GPU, so it works with the window
    /// occluded or unfocused and never disturbs whatever the human is doing.
    /// Intercepted by the socket decoder.
    Screenshot {
        #[serde(default)]
        path: Option<String>,
    },

    Chat {
        kind: u8,
        text: String,
    },

    Tell {
        to: String,
        text: String,
    },

    Action {
        target_id: u32,
        target_index: u16,
        kind: ActionKind,
    },

    /// c2s 0x05D MOTION: perform a canned emote. `target_id`/`target_index`
    /// `None` = untargeted (wire UniqueNo/ActIndex 0, per
    /// research/XiPackets/world/client/0x005D). `mode` is the EmoteMode byte
    /// (`ffxi_proto::map::emote::mode`), `param` the emote extra (bell note,
    /// job id + 0x1E, dance variant…).
    Emote {
        emote_id: u8,
        mode: u8,
        param: u16,
        target_id: Option<u32>,
        target_index: Option<u16>,
    },

    /// c2s 0x119 EMOTE_LIST: header-only request for the job-emote/chair
    /// unlock bitfields (answered by s2c 0x11A).
    RequestEmoteList,

    ReturnToHomePoint,

    SetFps {
        max: u32,
    },

    Follow {
        target_id: u32,
        distance: f32,
    },

    Engage {
        target_id: u32,
    },

    /// Client lock-on (target lock) state. The reactor keeps the engaged target
    /// squared up only while locked; clearing it frees the heading so the player
    /// can turn away mid-fight. Reactor-only (never forwarded to the server), and
    /// only the viewer emits it — headless agents never do, so the reactor's
    /// default keeps facing so auto-attack lands.
    SetTargetLock {
        locked: bool,
    },

    PathTo {
        x: f32,
        y: f32,
        z: f32,
        force: bool,
    },

    Cancel,

    UseItem {
        container: u8,
        slot: u8,
        item_no: u32,
        target_id: u32,
        target_index: u16,
    },

    Equip {
        container: u8,

        container_index: u8,

        equip_slot: u8,
    },

    /// Ask the server to consolidate same-id partial stacks in a container
    /// (retail's inventory "Sort"). `container` is the LSB CONTAINER_ID
    /// (LOC_INVENTORY = 0). See GP_CLI_COMMAND_ITEM_STACK (0x03A).
    StackInventory {
        container: u8,
    },

    /// One delivery box request (c2s 0x04D PBX). The session auto-sequences
    /// the retail flows (open → Work → Check → Recv/Confirm) on the server's
    /// 0x04B replies; explicit ops here are for agents driving it directly.
    DeliveryBox {
        #[serde(flatten)]
        op: DeliveryBoxOp,
    },

    /// Take an incoming parcel from inbox `slot`. The session runs the retail
    /// Accept→Get chain (`DeliveryBoxSession::request_take`) rather than a raw
    /// single op, since Get depends on the Accept ack.
    DeliveryTake {
        slot: u8,
    },

    /// Move `quantity` of the item at `from_container`/`from_slot` into
    /// `to_container` via c2s 0x029 ITEM_MOVE. `to_slot: None` lets the server
    /// pick a free slot; `Some(slot)` requests a same-id stack merge, which the
    /// server honors only when the FULL stack moves — a partial quantity always
    /// splits into a server-picked slot (0x029_item_move.cpp process).
    MoveItem {
        quantity: u32,
        from_container: u8,
        to_container: u8,
        from_slot: u8,
        to_slot: Option<u8>,
    },

    BankWhenFull {
        threshold: u8,
        mog_house_zoneline: u32,
    },

    ShopBuy {
        shop_no: u16,
        shop_index: u8,
        qty: u32,
    },

    /// Appraise `qty` of the LOC_INVENTORY item in slot `item_index` for sale to an
    /// NPC shop (0x084 SHOP_SELL_REQ); the server replies with the unit price (0x03D).
    ShopSellReq {
        qty: u32,
        item_no: u16,
        item_index: u8,
    },

    /// Confirm the pending sell appraisal (0x085 SHOP_SELL_SET).
    ShopSellConfirm,

    CheckTarget {
        target_id: u32,
        target_index: u16,
        kind: CheckKind,
    },

    /// Browse a PC's bazaar (c2s 0x105). The server answers with one s2c 0x105
    /// per priced slot and refuses while we still hold another bazaar open, so
    /// the session sends `CloseBazaar` first when one is already open.
    OpenBazaar {
        target_id: u32,
        target_index: u16,
    },

    /// Buy `quantity` of the browsed bazaar's row `index` (c2s 0x106). The
    /// server caps quantity at 99 (0x106_bazaar_buy.cpp validate).
    BuyBazaarItem {
        index: u8,
        quantity: u32,
    },

    /// Leave the bazaar we are browsing (c2s 0x104).
    CloseBazaar,

    Heal {
        mode: HealMode,
    },

    /// Begin fishing (`/fish`). The reactor casts and then drives the mini-game protocol.
    Fish,

    /// Player/agent input during the fishing mini-game (arrow reactions, hook, cancel).
    FishingInput {
        input: FishingInput,
    },

    /// Internal: emitted by the reactor's fishing machine; the session turns it into a
    /// c2s 0x110 GP_CLI_COMMAND_FISHING_2 packet.
    FishingRequest {
        mode: FishingMode,
        para: i32,
        para2: i32,
    },

    /// c2s 0x0F4 TRACKING_LIST (SendFlg = 1): request the wide-scan list. The
    /// server answers with 0x0F6 ListStart, a run of 0x0F4 entries, then 0x0F6
    /// ListEnd (vendor/server/src/map/packets/c2s/0x0f4_tracking_list.h).
    WidescanRequest,

    /// c2s 0x0F5 TRACKING_START: begin tracking `act_index`; the server then
    /// streams 0x0F5 position updates
    /// (vendor/server/src/map/packets/c2s/0x0f5_tracking_start.h).
    WidescanTrack {
        act_index: u16,
    },

    /// c2s 0x0F6 TRACKING_END: stop tracking
    /// (vendor/server/src/map/packets/c2s/0x0f6_tracking_end.h).
    WidescanEnd,

    /// Fetch a category's catalog from the search server (TCP_AH_REQUEST).
    /// `sorts` are the server-side ORDER BY params
    /// (`ffxi_proto::search::SORT_*`); empty = retail's "natural" ascending
    /// item-id order. Runs on its own task; one search in flight at a time.
    AhBrowse {
        category: u8,
        #[serde(default)]
        sorts: Vec<u32>,
    },

    /// Fetch an item's last-10-sales history from the search server
    /// (TCP_AH_HISTORY_SINGLE/_STACK).
    AhHistory {
        item_id: u16,
        stack: bool,
    },

    /// Bid on `item_id` (c2s 0x04E Bid). The server fills the cheapest
    /// matching listing priced <= `price`, or answers 0xC5.
    AhBid {
        item_id: u16,
        stack: bool,
        price: u32,
    },

    /// Request a listing-fee quote (c2s 0x04E AskCommit; the asking price
    /// rides Commission, ranged 1..=999_999_999 by GP_CLI_COMMAND_AUC::validate).
    /// `AhSellConfirm` completes the listing once the quote lands.
    AhSell {
        inventory_slot: u8,
        stack: bool,
        price: u32,
    },

    /// Complete a pending listing: LotIn with the stored quote and asking
    /// price. Ignored (with an error event) while no quote is pending.
    AhSellConfirm,

    /// Open Sales Status (c2s 0x04E Info; the server re-streams the 7 slots).
    AhSalesStatus,

    /// Cancel the sale in sales-status `slot` (c2s 0x04E LotCancel).
    AhCancelSale {
        slot: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Check,

    CheckName,

    CheckParam,
}

impl CheckKind {
    pub fn as_u8(self) -> u8 {
        match self {
            CheckKind::Check => 0,
            CheckKind::CheckName => 1,
            CheckKind::CheckParam => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealMode {
    Toggle,

    On,

    Off,
}

impl HealMode {
    pub fn as_u32(self) -> u32 {
        match self {
            HealMode::Toggle => 0,
            HealMode::On => 1,
            HealMode::Off => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReqLogoutKind {
    LogoutToggle,

    LogoutOn,

    LogoutOff,

    ShutdownToggle,

    ShutdownOn,

    ShutdownOff,
}

impl ReqLogoutKind {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MogHouseExit {
    /// Mode-0 "area you entered from"; `exit_bit` echoes the 0x00A MyRoomExitBit
    /// (retail derives it from the current zone — research/XiPackets/world/client/
    /// 0x005E; LSB's mode-0 path never reads it, and 0 = Default is in the
    /// MYROOMEXITBIT validator enum).
    Home {
        #[serde(default)]
        exit_bit: u8,
    },

    Sandoria {
        slot: u8,
    },

    Bastok {
        slot: u8,
    },

    Windurst {
        slot: u8,
    },

    Jeuno {
        slot: u8,
    },

    Whitegate {
        slot: u8,
    },

    Adoulin {
        slot: u8,
    },

    Mog1F,

    Mog2F,

    MogGarden,
}

impl MogHouseExit {
    /// Inverse of `wire_pair` for the city district exits (LSB MYROOMEXITBIT
    /// 1..=5, 9); any other bit is the mode-0 `Home` exit echoing that bit.
    pub fn from_bit_slot(bit: u8, slot: u8) -> Self {
        match bit {
            1 => MogHouseExit::Sandoria { slot },
            2 => MogHouseExit::Bastok { slot },
            3 => MogHouseExit::Windurst { slot },
            4 => MogHouseExit::Jeuno { slot },
            5 => MogHouseExit::Whitegate { slot },
            9 => MogHouseExit::Adoulin { slot },
            _ => MogHouseExit::Home { exit_bit: bit },
        }
    }

    pub fn wire_pair(self) -> (u8, u8) {
        match self {
            MogHouseExit::Home { exit_bit } => (exit_bit, 0),
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

/// The mode byte of a c2s 0x110 GP_CLI_COMMAND_FISHING_2 request.
/// vendor/server/src/map/packets/c2s/0x110_fishing_2.h
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FishingMode {
    /// The cast has settled; ask the server whether anything bit. para=0, para2=0.
    CheckHook = 2,
    /// The mini-game is over; report the outcome. para/para2 encode how it ended.
    EndMiniGame = 3,
    /// The resolution animation finished; ask the server to release the fishing lock.
    Release = 4,
    /// Time is nearly up; let the server warn the player. para=remaining time.
    PotentialTimeout = 5,
}

/// Player/agent input fed to the fishing mini-game state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FishingInput {
    /// Set the hook once a fish bites (Enter on retail).
    Hook,
    /// React to the on-screen arrow.
    Left,
    Right,
    /// Abandon the cast / mini-game (movement or Escape on retail).
    Cancel,
}

/// The fish stats from a s2c 0x115 GP_SERV_COMMAND_FISH, normalized into the values the
/// client mini-game uses. Mirrors [`ffxi_proto::decode::FishPacket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FishParams {
    pub stamina: u16,
    pub arrow_delay: u16,
    pub regen: u16,
    pub move_frequency: u16,
    pub arrow_damage: u16,
    pub arrow_regen: u16,
    pub time: u16,
    pub angler_sense: u8,
    pub intuition: u32,
}

impl From<ffxi_proto::decode::FishPacket> for FishParams {
    fn from(p: ffxi_proto::decode::FishPacket) -> Self {
        Self {
            stamina: p.stamina,
            arrow_delay: p.arrow_delay,
            regen: p.regen,
            move_frequency: p.move_frequency,
            arrow_damage: p.arrow_damage,
            arrow_regen: p.arrow_regen,
            time: p.time,
            angler_sense: p.angler_sense,
            intuition: p.intuition,
        }
    }
}

/// The on-screen arrow during the active mini-game state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FishingArrow {
    /// The direction the player must press to land the hit.
    pub left: bool,
    /// Golden arrows (driven by intuition) deal more stamina damage.
    pub golden: bool,
}

/// Which "something caught the hook" line the server sent. Retail labels the
/// mini-game bar off it (research/xim FishHppUi.kt); nothing in 0x115 carries
/// the size, so this is the only signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FishSize {
    Small,
    Large,
}

/// The self player's fishing state, as a view for the renderer/HUD. `None` when not
/// fishing. The reactor's fishing machine is the authoritative owner; this is the
/// projection it publishes through the event folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfFishing {
    /// Macro-state phase 0..=6 for self pose selection (see `ffxi_actor::fishing_clip`).
    pub phase: u8,
    /// The hooked fish's parameters, present once a fish bites.
    pub fish: Option<FishParams>,
    /// Current fish stamina, for the HUD bar (clamped to the fish's max).
    pub fish_hp: u16,
    /// The arrow the player must currently react to, if any.
    pub arrow: Option<FishingArrow>,
    /// Set by the hook message, which LSB pushes just before 0x115
    /// (vendor/server/src/map/utils/fishingutils.cpp `SendHookResponse`).
    pub size: Option<FishSize>,
}

impl SelfFishing {
    /// A fresh cast at `phase`, before anything has bitten.
    pub fn starting(phase: u8) -> Self {
        Self {
            phase,
            fish: None,
            fish_hp: 0,
            arrow: None,
            size: None,
        }
    }
}

/// The self player's in-flight cast/action, as a serializable view for the cast
/// bar. `None` when idle. The reactor's `CastInFlight` machine owns the truth
/// and republishes this each tick with a freshly-computed `elapsed_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfCasting {
    /// Resolved display name of the spell/ability being performed.
    pub name: String,
    /// Milliseconds elapsed since the action started (reactor-computed).
    pub elapsed_ms: u32,
    /// Total expected duration (spell castTime, or JA/WS/RA animation lock).
    pub total_ms: u32,
    /// Set once interrupted (moved mid-cast, paralyzed, server reject); the bar
    /// flashes then clears.
    pub interrupted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionKind {
    Talk,

    Attack,

    CastMagic {
        spell_id: u32,
        pos_x: f32,
        pos_y: f32,
        pos_z: f32,
    },

    AttackOff,

    Help,

    Weaponskill {
        skill_id: u32,
    },

    JobAbility {
        ability_id: u32,
    },

    HomepointMenu {
        status_id: u32,
    },

    Assist,

    RaiseMenu {
        accept: bool,
    },

    Fish,

    ChangeTarget,

    Shoot,

    ChocoboDig,

    Dismount,

    TractorMenu {
        accept: bool,
    },

    SendResRdy,

    Quarry,

    Sprint,

    Scout,

    Blockaid {
        status_id: u32,
    },

    MonsterSkill {
        skill_id: u32,
    },

    Mount {
        mount_id: u32,
    },
}

impl ActionKind {
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

            _ => {}
        }
    }

    /// Cast-bar plan for this action: the resolved spell name and its server
    /// cast time (ms), present only for spells with a non-zero cast time. Instant
    /// spells and non-spell actions have no cast bar (retail shows a bar only
    /// while a spell winds up).
    /// `dat_cast_ms` is the retail client's own cast time (from the spell DAT); when
    /// present it wins, since the real client displays and locks on that value. Falls
    /// back to the LSB-scraped cast time when the DAT is unavailable.
    pub fn cast_bar(&self, dat_cast_ms: Option<u32>) -> Option<(String, u32)> {
        match self {
            ActionKind::CastMagic { spell_id, .. } => {
                let ms = self.spell_cast_ms(*spell_id, dat_cast_ms)?;
                let name = ffxi_vocab::spell_names::lookup(*spell_id as u16)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("spell #{spell_id}"));
                Some((name, ms))
            }
            _ => None,
        }
    }

    /// How long the reactor should refuse a new action after issuing this one:
    /// a spell's cast time, or the fixed animation lock for instant JA/WS/ranged.
    /// `None` for actions that impose no lock (movement/menu/etc).
    pub fn action_lock_ms(&self, dat_cast_ms: Option<u32>) -> Option<u32> {
        match self {
            ActionKind::CastMagic { spell_id, .. } => self.spell_cast_ms(*spell_id, dat_cast_ms),
            ActionKind::JobAbility { .. } | ActionKind::Weaponskill { .. } | ActionKind::Shoot => {
                Some(INSTANT_ACTION_LOCK_MS)
            }
            _ => None,
        }
    }

    fn spell_cast_ms(&self, spell_id: u32, dat_cast_ms: Option<u32>) -> Option<u32> {
        dat_cast_ms
            .or_else(|| ffxi_vocab::cast_time::spell_cast_time_ms(spell_id as u16).map(u32::from))
            .filter(|ms| *ms > 0)
    }
}

/// Client-side animation lock for instant actions (job abilities, weapon skills,
/// ranged attack): retail plays a short action animation during which the next
/// command is ignored. A feel tuning the LSB data doesn't expose as one value.
pub const INSTANT_ACTION_LOCK_MS: u32 = 1000;

#[cfg(test)]
mod tests;
