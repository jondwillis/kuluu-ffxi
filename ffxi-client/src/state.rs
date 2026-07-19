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

    /// Live LSB STATUS_TYPE byte, refreshed every update (the server writes it
    /// unconditionally, unlike npc_state's UPDATE_HP-gated fields). 0 = NORMAL.
    /// Authoritative for target eligibility; see `ffxi_viewer_wire::Entity`.
    #[serde(default)]
    pub status: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLine {
    pub channel: ChatChannel,
    pub sender: String,
    pub text: String,

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
    pub chat: Vec<ChatLine>,
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

    #[serde(default)]
    pub myroom: Option<MyRoomInfo>,

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
}

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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeliveryBoxState {
    /// Which box the server currently has open for us, if any.
    pub open: Option<DeliveryBoxNo>,
    pub slots: Vec<Option<DeliveryItem>>,
    /// Last Check answer: items still queued beyond the 8 visible slots.
    pub queued: u8,
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

    #[serde(default)]
    pub in_mog_house: bool,
}

const CHAT_HISTORY_CAP: usize = 256;

impl SessionState {
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

                self.logout_countdown = None;
                self.death_homepoint_secs = None;

                self.entities.clear();
                self.party.clear();

                self.current_weather = None;
                self.check_result = None;
                true
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
                if let Some(existing) = self.entities.iter_mut().find(|e| e.id == entity.id) {
                    let preserved_name = entity.name.clone().or_else(|| existing.name.clone());
                    let merged_kind = merge_kind(existing.kind, entity.kind);

                    let preserved_hp_pct = entity.hp_pct.or(existing.hp_pct);

                    let preserved_look = entity.look.or(existing.look);
                    let preserved_npc_state = entity.npc_state.or(existing.npc_state);

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
                        pos: preserved_pos,
                        heading: preserved_heading,
                        speed: preserved_speed,
                        speed_base: preserved_speed_base,
                        face_target: preserved_face_target,
                        ..entity.clone()
                    };
                    if *existing == merged {
                        false
                    } else {
                        *existing = merged;
                        true
                    }
                } else {
                    self.entities.push(entity.clone());
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
                self.chat.push(line.clone());
                if self.chat.len() > CHAT_HISTORY_CAP {
                    let drop = self.chat.len() - CHAT_HISTORY_CAP;
                    self.chat.drain(0..drop);
                }
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
                    || self.logout_countdown.is_some();
                self.stage = Stage::Disconnected;
                self.diagnostics.stage = Some(Stage::Disconnected);

                self.logout_countdown = None;
                changed
            }

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
                true
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
                    let merged = PartyMember {
                        name: preserved_name,
                        is_party_leader: preserved_leader,
                        is_alliance_leader: preserved_alliance,
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
                    DeliveryBoxUpdate::RecipientCheck { .. } | DeliveryBoxUpdate::Failed { .. } => {
                        return false
                    }
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
            } => {
                let r = self.check_result_mut(*target_id, *act_index);
                let next = (*main_job, *sub_job, *main_job_lv, *sub_job_lv, *master_lv);
                let changed = (
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
                changed
            }
            AgentEvent::CheckCleared => {
                let changed = self.check_result.is_some();
                self.check_result = None;
                changed
            }
            AgentEvent::EventStart { .. } | AgentEvent::KeyRotated { .. } => false,
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
                self.chat.push(ChatLine {
                    channel: ChatChannel::System,
                    sender: "<shop>".into(),
                    text: format!(
                        "Appraisal: slot {item_index} x{count} sells for {price} gil each \
                         — `/sell confirm` to accept"
                    ),
                    server_ts: 0,
                });
                if self.chat.len() > CHAT_HISTORY_CAP {
                    let drop = self.chat.len() - CHAT_HISTORY_CAP;
                    self.chat.drain(0..drop);
                }
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
            AgentEvent::DeathTimerUpdated {
                seconds_until_homepoint,
            } => {
                let changed = self.death_homepoint_secs != *seconds_until_homepoint;
                self.death_homepoint_secs = *seconds_until_homepoint;
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
            // Machine inputs (consumed by the reactor, not the rendered projection).
            AgentEvent::FishingCast { .. }
            | AgentEvent::FishingServerPhase { .. }
            | AgentEvent::FishingEnded => false,
            AgentEvent::FishHooked { params } => {
                let f = self.self_fishing.get_or_insert(SelfFishing {
                    phase: 1,
                    fish: None,
                    fish_hp: 0,
                    arrow: None,
                });
                let changed = f.fish != Some(*params) || f.fish_hp != params.stamina;
                f.fish = Some(*params);
                f.fish_hp = params.stamina;
                changed
            }
            AgentEvent::FishingPhaseChanged { phase } => match phase {
                Some(p) => {
                    let changed = self.self_fishing.map(|f| f.phase) != Some(*p);
                    self.self_fishing
                        .get_or_insert(SelfFishing {
                            phase: *p,
                            fish: None,
                            fish_hp: 0,
                            arrow: None,
                        })
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
    EventEnded,

    ActionStarted {
        actor_id: u32,
        action_id: u32,
        action_kind: u8,
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

    /// Raw self animation phase straight from the 0x037 byte (machine input for the
    /// resolution/release handshake). Distinct from `FishingPhaseChanged`, which is the
    /// reactor machine's published view.
    FishingServerPhase {
        phase: Option<u8>,
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
    /// and levels (zeroed while the target is /anon).
    CheckGeneralReceived {
        target_id: u32,
        act_index: u16,
        main_job: u8,
        sub_job: u8,
        main_job_lv: u8,
        sub_job_lv: u8,
        master_lv: u8,
    },

    /// Outbound /check dispatched: drop the previous target's accumulated result.
    CheckCleared,
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

    /// Mark key items seen — c2s 0x064 GP_CLI_COMMAND_SCENARIOITEM with the
    /// table's full updated LookItemFlag bitset
    /// (vendor/server/src/map/packets/c2s/0x064_scenarioitem.cpp ORs each set bit).
    MarkKeyItemsSeen {
        table_index: u16,
        ids: Vec<u16>,
    },

    EndEvent,

    EndEventChoice {
        event_id: u32,
        act_index: u16,
        event_num: u16,
        choice: u32,
    },

    Disconnect,

    ReqLogout {
        kind: ReqLogoutKind,
    },

    Snapshot,

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_fold_sets_and_zone_change_clears() {
        let mut s = SessionState::default();
        assert_eq!(s.current_weather, None);
        s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 6 });
        assert_eq!(s.current_weather, Some(6));
        s.apply_event(&AgentEvent::ZoneChanged {
            from: Some(230),
            to: 231,
            myroom: None,
            mog_zone_flag: false,
        });
        assert_eq!(s.current_weather, None);
    }

    #[test]
    fn zone_change_sets_and_clears_myroom() {
        let mut s = SessionState {
            char_id: Some(0xCAFE),
            ..Default::default()
        };
        let room = MyRoomInfo {
            model: 257,
            sub_map: 0,
            exit_bit: 1,
        };
        s.apply_event(&AgentEvent::ZoneChanged {
            from: None,
            to: 230,
            myroom: Some(room),
            mog_zone_flag: false,
        });
        assert_eq!(s.myroom, Some(room));
        assert!(
            s.self_in_mog_house(),
            "myroom must drive self_in_mog_house before any party attrs arrive"
        );

        s.apply_event(&AgentEvent::ZoneChanged {
            from: Some(230),
            to: 230,
            myroom: None,
            mog_zone_flag: false,
        });
        assert_eq!(s.myroom, None);
        assert!(!s.self_in_mog_house());
    }

    #[test]
    fn job_info_and_2f_unlock_fold() {
        let mut s = SessionState::default();
        let mut job_levels = [0u8; ffxi_proto::decode::JobInfo::MAX_JOBTYPE];
        job_levels[1] = 75;
        let info = JobInfoState {
            mjob_no: 1,
            sjob_no: 3,
            unlocked: 0b1011,
            sub_job_unlocked: true,
            job_levels,
        };
        s.apply_event(&AgentEvent::JobInfoUpdated { info });
        assert_eq!(s.job_info, Some(info));

        assert_eq!(s.mh_2f_unlocked, None);
        s.apply_event(&AgentEvent::MogHouse2fUnlockUpdated { unlocked: true });
        assert_eq!(s.mh_2f_unlocked, Some(true));
    }

    #[test]
    fn equip_updated_index_zero_clears_slot() {
        let mut s = SessionState::default();
        // Equip something in the waist slot (10).
        s.apply_event(&AgentEvent::EquipUpdated {
            slot: 10,
            container: 0,
            container_index: 7,
        });
        assert_eq!(
            s.equipment[10],
            Some(EquippedRef {
                container: 0,
                container_index: 7
            })
        );
        // Server reports an unequipped slot as inventory index 0 (= Gil); the
        // slot must clear, not point at inventory slot 0.
        s.apply_event(&AgentEvent::EquipUpdated {
            slot: 10,
            container: 0,
            container_index: 0,
        });
        assert_eq!(s.equipment[10], None, "index 0 = empty, not Gil");
    }

    #[test]
    fn key_items_merge_across_tables_and_replace_in_place() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::KeyItemsUpdated {
            table_index: 0,
            ids: vec![1, 5],
            seen_ids: vec![1],
        });
        s.apply_event(&AgentEvent::KeyItemsUpdated {
            table_index: 1,
            ids: vec![KEY_ITEMS_PER_TABLE as u16],
            seen_ids: Vec::new(),
        });
        assert_eq!(s.key_items, vec![1, 5, KEY_ITEMS_PER_TABLE as u16]);
        assert_eq!(s.key_items_seen, vec![1]);

        s.apply_event(&AgentEvent::KeyItemsUpdated {
            table_index: 0,
            ids: vec![5],
            seen_ids: vec![5],
        });
        assert_eq!(s.key_items, vec![5, KEY_ITEMS_PER_TABLE as u16]);
        assert_eq!(s.key_items_seen, vec![5], "table 0 seen set replaced");
    }

    #[test]
    fn key_items_seen_refresh_keeps_other_tables() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::KeyItemsUpdated {
            table_index: 1,
            ids: vec![KEY_ITEMS_PER_TABLE as u16 + 2],
            seen_ids: vec![KEY_ITEMS_PER_TABLE as u16 + 2],
        });
        let changed = s.apply_event(&AgentEvent::KeyItemsUpdated {
            table_index: 0,
            ids: vec![3],
            seen_ids: vec![3],
        });
        assert!(changed);
        assert_eq!(s.key_items_seen, vec![3, KEY_ITEMS_PER_TABLE as u16 + 2]);

        let unchanged = s.apply_event(&AgentEvent::KeyItemsUpdated {
            table_index: 0,
            ids: vec![3],
            seen_ids: vec![3],
        });
        assert!(!unchanged, "identical refresh must not report a change");
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

    /// The flattened op tag keeps delivery box commands one-level JSON for
    /// headless agents: {"cmd":"delivery_box","op":"set",...}.
    #[test]
    fn delivery_box_command_roundtrip() {
        let line = r#"{"cmd":"delivery_box","op":"set","slot":0,"inventory_slot":11,"quantity":1,"recipient":"Atti"}"#;
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        match &cmd {
            AgentCommand::DeliveryBox {
                op:
                    DeliveryBoxOp::Set {
                        slot,
                        inventory_slot,
                        quantity,
                        recipient,
                    },
            } => {
                assert_eq!((*slot, *inventory_slot, *quantity), (0, 11, 1));
                assert_eq!(recipient, "Atti");
            }
            _ => panic!("wrong variant: {cmd:?}"),
        }
        let back = serde_json::to_string(&cmd).unwrap();
        assert_eq!(serde_json::from_str::<AgentCommand>(&back).unwrap(), cmd);

        let line = r#"{"cmd":"delivery_box","op":"post_open"}"#;
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        assert!(matches!(
            cmd,
            AgentCommand::DeliveryBox {
                op: DeliveryBoxOp::PostOpen
            }
        ));

        let line = r#"{"cmd":"delivery_box","op":"check","box_no":"incoming"}"#;
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        assert!(matches!(
            cmd,
            AgentCommand::DeliveryBox {
                op: DeliveryBoxOp::Check {
                    box_no: DeliveryBoxNo::Incoming
                }
            }
        ));
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

        let from_attr = PartyMember {
            id: 42,
            act_index: 7,
            name: None,
            hp: 1500,
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
                face_target: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
                npc_state: None,
                status: 0,
            },
            pos_present: true,
        });
        assert_eq!(s.entities.len(), 1);

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
                face_target: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
                npc_state: None,
                status: 0,
            },
            pos_present: true,
        });
        assert_eq!(s.entities.len(), 1, "upsert must not duplicate by id");
        assert_eq!(s.entities[0].pos.x, 5.0, "upsert must overwrite");

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
            myroom: None,
            mog_zone_flag: false,
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

        s.apply_event(&AgentEvent::Disconnected {
            reason: "test".into(),
        });
        assert_eq!(s.stage, Stage::Disconnected);
    }

    #[test]
    fn merge_kind_specialized_wins_over_other() {
        use EntityKind::*;

        assert_eq!(merge_kind(Pc, Other), Pc);
        assert_eq!(merge_kind(Npc, Other), Npc);
        assert_eq!(merge_kind(Mob, Other), Mob);
        assert_eq!(merge_kind(Pet, Other), Pet);

        assert_eq!(merge_kind(Other, Pet), Pet);
        assert_eq!(merge_kind(Other, Npc), Npc);

        assert_eq!(merge_kind(Npc, Pet), Pet);
        assert_eq!(merge_kind(Pet, Npc), Npc);

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
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            npc_state: None,
            status: 0,
        }
    }

    #[test]
    fn entity_upserted_preserves_name_across_attr_only_update() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(42, Some("Sigli-Sea"), EntityKind::Npc),
            pos_present: true,
        });
        assert_eq!(s.entities[0].name.as_deref(), Some("Sigli-Sea"));

        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(42, None, EntityKind::Npc),
            pos_present: true,
        });
        assert_eq!(
            s.entities[0].name.as_deref(),
            Some("Sigli-Sea"),
            "name must persist across attr-only update"
        );
    }

    #[test]
    fn entity_upserted_status_refreshes_on_pos_only_tick() {
        let mut s = SessionState::default();
        let mut ent = make_test_entity(42, Some("Antlion"), EntityKind::Mob);
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: ent.clone(),
            pos_present: true,
        });
        assert_eq!(s.entities[0].status, 0, "spawns NORMAL");

        ent.npc_state = None;
        ent.status = 3;
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: ent,
            pos_present: true,
        });
        assert_eq!(
            s.entities[0].status, 3,
            "a pos-only tick (no npc_state) must still refresh STATUS_TYPE"
        );
    }

    #[test]
    fn entity_upserted_preserves_position_when_pos_absent() {
        let mut s = SessionState::default();
        let mut ent = make_test_entity(42, Some("Tunnel Worm"), EntityKind::Mob);
        ent.pos = Vec3 {
            x: 123.0,
            y: 4.0,
            z: -89.0,
        };
        ent.heading = 200;
        ent.speed = 40;
        ent.speed_base = 40;
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: ent,
            pos_present: true,
        });
        assert_eq!(s.entities[0].pos.x, 123.0);

        let mut hp_only = make_test_entity(42, None, EntityKind::Mob);
        hp_only.pos = Vec3::default();
        hp_only.heading = 0;
        hp_only.speed = 0;
        hp_only.speed_base = 0;
        hp_only.hp_pct = Some(75);
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: hp_only,
            pos_present: false,
        });
        assert_eq!(
            s.entities[0].pos,
            Vec3 {
                x: 123.0,
                y: 4.0,
                z: -89.0
            },
            "position must persist across a non-UPDATE_POS tick (no teleport to origin)"
        );
        assert_eq!(s.entities[0].heading, 200, "heading must persist too");
        assert_eq!(s.entities[0].speed, 40, "speed must persist too");
        assert_eq!(
            s.entities[0].hp_pct,
            Some(75),
            "the HP this tick *did* carry must still apply"
        );

        let mut moved = make_test_entity(42, None, EntityKind::Mob);
        moved.pos = Vec3 {
            x: 200.0,
            y: 4.0,
            z: -50.0,
        };
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: moved,
            pos_present: true,
        });
        assert_eq!(
            s.entities[0].pos.x, 200.0,
            "a genuine position update must overwrite, not get preserved"
        );
    }

    #[test]
    fn entity_upserted_preserves_hp_pct_across_position_only_update() {
        let mut s = SessionState::default();
        let mut ent = make_test_entity(42, Some("Worker Bee"), EntityKind::Npc);
        ent.hp_pct = Some(50);
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: ent,
            pos_present: true,
        });
        assert_eq!(s.entities[0].hp_pct, Some(50));

        let mut pos_only = make_test_entity(42, None, EntityKind::Npc);
        pos_only.hp_pct = None;
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: pos_only,
            pos_present: true,
        });
        assert_eq!(
            s.entities[0].hp_pct,
            Some(50),
            "hp_pct must persist across UPDATE_POS-only follow-up (no UPDATE_HP bit set)"
        );

        let mut died = make_test_entity(42, None, EntityKind::Npc);
        died.hp_pct = Some(0);
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: died,
            pos_present: true,
        });
        assert_eq!(
            s.entities[0].hp_pct,
            Some(0),
            "Some(0) (mob died) must overwrite, not get preserved as Some(50)"
        );
    }

    #[test]
    fn entity_upserted_preserves_look_across_position_only_update() {
        use ffxi_proto::decode::LookData;
        let mut s = SessionState::default();
        let mut ent = make_test_entity(42, Some("Jonisbarius"), EntityKind::Pc);
        let look = LookData::Equipped {
            face: 3,
            race: 3,
            head: 0x1000,
            body: 0x2004,
            hands: 0x3000,
            legs: 0x4000,
            feet: 0x5000,
            main: 0x6000,
            sub: 0x7000,
            ranged: 0x8000,
        };
        ent.look = Some(look);
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: ent,
            pos_present: true,
        });
        assert!(matches!(
            s.entities[0].look,
            Some(LookData::Equipped { race: 3, .. })
        ));

        let mut pos_only = make_test_entity(42, None, EntityKind::Pc);
        pos_only.look = None;
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: pos_only,
            pos_present: true,
        });
        assert!(
            matches!(s.entities[0].look, Some(LookData::Equipped { race: 3, .. })),
            "look must persist across position-only refresh (no look bits set)"
        );

        let mut changed = make_test_entity(42, None, EntityKind::Pc);
        changed.look = Some(LookData::Standard { modelid: 99 });
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: changed,
            pos_present: true,
        });
        assert!(
            matches!(s.entities[0].look, Some(LookData::Standard { modelid: 99 })),
            "Some(new_look) must overwrite, not get preserved as the prior Equipped value"
        );
    }

    #[test]
    fn entity_upserted_specialized_kind_resists_demotion_to_other() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(7, Some("Stout Servitor"), EntityKind::Npc),
            pos_present: true,
        });
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(7, Some("Stout Servitor"), EntityKind::Other),
            pos_present: true,
        });
        assert_eq!(s.entities[0].kind, EntityKind::Npc);
    }

    #[test]
    fn entity_patched_by_id_sets_name_on_existing_entity() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(99, None, EntityKind::Other),
            pos_present: true,
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
        let mut s = SessionState::default();
        let mut ent = make_test_entity(0xABCD, None, EntityKind::Other);
        ent.act_index = 0x07A5;
        s.apply_event(&AgentEvent::EntityUpserted {
            entity: ent,
            pos_present: true,
        });
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

        assert_eq!(s.name_misses.front().unwrap().unique_no, 5);

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

        assert!(s.contains("name_bit_set_extraction_failed"));
    }

    #[test]
    fn entity_patched_for_unknown_entity_is_dropped() {
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

        assert!(s.self_position().is_none());

        s.apply_event(&AgentEvent::Connected {
            account_id: 1,
            char_id: 99,
            character: "Self".into(),
            zone_id: 230,
        });

        assert!(s.self_position().is_none());

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
                face_target: 0,
                claim_id: 0,
                speed: 40,
                speed_base: 40,
                look: None,
                npc_state: None,
                status: 0,
            },
            pos_present: true,
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
        caps[0] = 80;
        caps[1] = 200;
        caps[5] = 30;
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::Capacities { capacities: caps },
        });
        assert_eq!(s.inventory.containers[&0].capacity, 80);
        assert_eq!(s.inventory.containers[&1].capacity, 200);
        assert_eq!(s.inventory.containers[&5].capacity, 30);

        // A later 0 must land too — it is LSB's "container disabled" sentinel
        // (e.g. a lapsed Mog Locker lease), not an absence of data.
        let mut caps = vec![0u16; 18];
        caps[0] = 80;
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::Capacities { capacities: caps },
        });
        assert_eq!(
            s.inventory.containers[&5].capacity, 0,
            "capacity grants must not be sticky"
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

        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::SlotChanged { slot: slot.clone() },
        });
        assert_eq!(s.inventory.containers[&0].slots.len(), 1);
        assert_eq!(s.inventory.containers[&0].slots[0].quantity, 5);

        let mut updated = slot.clone();
        updated.quantity = 12;
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::SlotChanged { slot: updated },
        });
        assert_eq!(s.inventory.containers[&0].slots.len(), 1, "no duplication");
        assert_eq!(s.inventory.containers[&0].slots[0].quantity, 12);

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

        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::QuantityChanged {
                index: 7,
                quantity: 99,
                locked: false,
            },
        });

        assert!(
            s.inventory
                .containers
                .get(&0)
                .map(|c| c.slots.is_empty())
                .unwrap_or(true),
            "ITEM_NUM without prior ITEM_LIST drops"
        );

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
    fn check_result_accumulates_equipment_batches_and_general() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::CheckEquipReceived {
            target_id: 0xCAFE,
            act_index: 0x123,
            items: vec![(0, 17440), (4, 12511)],
        });
        s.apply_event(&AgentEvent::CheckEquipReceived {
            target_id: 0xCAFE,
            act_index: 0x123,
            items: vec![(15, 13465)],
        });
        s.apply_event(&AgentEvent::CheckGeneralReceived {
            target_id: 0xCAFE,
            act_index: 0x123,
            main_job: 1,
            sub_job: 13,
            main_job_lv: 75,
            sub_job_lv: 37,
            master_lv: 0,
        });
        let r = s.check_result.as_ref().expect("accumulated");
        assert_eq!(r.target_id, 0xCAFE);
        assert_eq!(r.equipped[0], Some(17440), "batch 1 Main");
        assert_eq!(r.equipped[4], Some(12511), "batch 1 Head");
        assert_eq!(r.equipped[15], Some(13465), "batch 2 Back");
        assert_eq!(r.equipped[1], None, "unsent slot stays empty");
        assert_eq!((r.main_job, r.main_job_lv), (1, 75));
        assert_eq!((r.sub_job, r.sub_job_lv), (13, 37));
    }

    #[test]
    fn check_result_resets_on_new_target_and_clears() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::CheckEquipReceived {
            target_id: 0xCAFE,
            act_index: 0x123,
            items: vec![(0, 17440)],
        });
        s.apply_event(&AgentEvent::CheckEquipReceived {
            target_id: 0xBEEF,
            act_index: 0x456,
            items: vec![(4, 12511)],
        });
        let r = s.check_result.as_ref().expect("new target");
        assert_eq!(r.target_id, 0xBEEF);
        assert_eq!(r.equipped[0], None, "old target's gear dropped");
        assert_eq!(r.equipped[4], Some(12511));

        s.apply_event(&AgentEvent::CheckCleared);
        assert!(
            s.check_result.is_none(),
            "outbound /check drops stale result"
        );

        s.apply_event(&AgentEvent::CheckEquipReceived {
            target_id: 0xBEEF,
            act_index: 0x456,
            items: vec![(4, 12511)],
        });
        s.apply_event(&AgentEvent::ZoneChanged {
            from: None,
            to: 230,
            myroom: None,
            mog_zone_flag: false,
        });
        assert!(s.check_result.is_none(), "zone change drops stale result");
    }

    #[test]
    fn check_equip_out_of_range_slot_is_ignored() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::CheckEquipReceived {
            target_id: 0xCAFE,
            act_index: 0x123,
            items: vec![(16, 17440), (0xFF, 1)],
        });
        let r = s.check_result.as_ref().expect("result created");
        assert!(r.equipped.iter().all(|c| c.is_none()));
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

        assert_eq!(s.chat[0].text, "msg 50");
    }

    #[test]
    fn apply_event_reports_real_mutations_only() {
        let mut s = SessionState::default();

        // Machine-input / notification-only events never mutate folded state.
        assert!(!s.apply_event(&AgentEvent::HumanReleased));
        assert!(!s.apply_event(&AgentEvent::FishingEnded));

        // Scalar fields: first fold mutates, identical resend is a no-op.
        assert!(s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 6 }));
        assert!(!s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 6 }));
        assert!(s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 7 }));

        assert!(s.apply_event(&AgentEvent::StageChanged {
            stage: Stage::Authenticating,
        }));
        assert!(!s.apply_event(&AgentEvent::StageChanged {
            stage: Stage::Authenticating,
        }));
    }

    #[test]
    fn apply_event_dedupes_identical_entity_upserts() {
        let mut s = SessionState::default();
        let entity = Entity {
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
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            npc_state: None,
            status: 0,
        };

        // First upsert inserts.
        assert!(s.apply_event(&AgentEvent::EntityUpserted {
            entity: entity.clone(),
            pos_present: true,
        }));
        // Byte-identical resend folds to a no-op.
        assert!(!s.apply_event(&AgentEvent::EntityUpserted {
            entity: entity.clone(),
            pos_present: true,
        }));
        // A real change signals again.
        assert!(s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                heading: 32,
                ..entity
            },
            pos_present: true,
        }));

        // Removing a missing entity is a no-op; removing a present one is not.
        assert!(!s.apply_event(&AgentEvent::EntityRemoved { id: 1234 }));
        assert!(s.apply_event(&AgentEvent::EntityRemoved { id: 999 }));
    }

    #[test]
    fn apply_event_dedupes_identical_self_position() {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::Connected {
            account_id: 1,
            char_id: 7,
            character: "Tester".into(),
            zone_id: 100,
        });

        let pos = Position {
            pos: Vec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            heading: 10,
            speed: 40,
            speed_base: 40,
        };
        // No self entity folded yet: position update touches nothing.
        assert!(!s.apply_event(&AgentEvent::PositionChanged { pos }));

        s.apply_event(&AgentEvent::EntityUpserted {
            entity: Entity {
                id: 7,
                act_index: 1,
                kind: EntityKind::Pc,
                name: Some("Tester".into()),
                pos: Vec3::default(),
                heading: 0,
                hp_pct: Some(100),
                bt_target_id: 0,
                face_target: 0,
                claim_id: 0,
                speed: 40,
                speed_base: 40,
                look: None,
                npc_state: None,
                status: 0,
            },
            pos_present: true,
        });

        // First real move mutates; the identical resend does not.
        assert!(s.apply_event(&AgentEvent::PositionChanged { pos }));
        assert!(!s.apply_event(&AgentEvent::PositionChanged { pos }));
    }
}
