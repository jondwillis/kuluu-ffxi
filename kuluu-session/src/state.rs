use std::collections::{HashMap, HashSet, VecDeque};
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

/// The wire speed decode lives in kuluu-snapshot so the render layer (which cannot depend on
/// kuluu-session at runtime) and the session reactor share one source of truth; re-exported here
/// for the existing `kuluu_session::state::{...}` call sites.
pub use kuluu_snapshot::speed::{
    move_speed_ratio, move_speed_yps, walk_speed_yps, AUTHORED_ANIM_RATE, BASE_PACKET_SPEED,
    MAX_MOVE_SPEED_YPS, MOUNTED_SPEED_MULTIPLIER, SPEED_TO_YPS, WALK_SPEED_DIVISOR,
    WALK_SPEED_SCALE,
};

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
    /// UPDATE_HP — vendor/server/src/map/packets/entity_update.cpp CEntityUpdatePacket::updateWith put
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

    /// `GP_SERV_CHAR_PC.MonstrosityFlags` (body 0x3A) of the last Model-block
    /// update. Written only under `SendFlg.Model` — vendor/server/src/map/packets/
    /// char_update.cpp `CCharUpdatePacket::updateWith` — so a General-block update
    /// does not refresh it; preserved across non-Model updates like `mount_id`.
    /// Drives the retail Monstrosity nameplate marker (0xAB).
    #[serde(skip)]
    pub monstrosity: Option<bool>,

    /// `Flags4.JobMasterFlag` of the last non-despawn 0x0D. Unlike the General
    /// words in `char_flags`, LSB writes it on every update, outside all SendFlg
    /// blocks (vendor/server/src/map/packets/char_update.cpp "Fields that are
    /// always checked if this isnt a despawn packet"), so it refreshes even on
    /// pos-only updates. PC-only; folded into `char_flags` at upsert time.
    #[serde(skip)]
    pub job_master_display: Option<bool>,
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
    /// (vendor/server/src/map/enums/chat_message_type.h CHAT_MESSAGE_TYPE MESSAGE_EMOTION).
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
    /// CLISTATUS exp_now/exp_next (s2c 0x061), fed to the `level` box under
    /// the persona panel.
    #[serde(default)]
    pub exp_now: i16,
    #[serde(default)]
    pub exp_next: i16,
    pub hp_max: u32,
    pub mp_max: u32,
    pub bp_base: [u16; 7],
    pub bonus: [i16; 7],
    pub attack: u16,
    pub defense: u16,
    pub resist: [i16; 8],
    pub ilvl: u8,
    /// `su_lv` (s2c 0x061 CLISTATUS su_lv) — superior level.
    #[serde(default)]
    pub su_lv: u8,
    /// bit 0 of s2c 0x061 `mastery_info.flags` — master-level play unlocked
    /// (gates the Status window's Master Levels row together with a level-99 main
    /// job; FFXiMain.dll retail-2026-09 `.text 0x148B20`).
    #[serde(default)]
    pub master_level_unlocked: bool,
    /// s2c 0x061 packet +0x60 — the per-slot equip-block mask the fifth CanEquip test
    /// intersects record slot masks against (FFXiMain.dll retail-2026-09 `.text 0xFD350`,
    /// stored to game state `+0x40DF0` at `.text 0xF0982`; refusal at `.text 0x132111`).
    #[serde(default)]
    pub equip_slot_block: u32,
    /// CLISTATUS `mjob_no` and the Unity fields `unity_info.Faction` / `unity_info.Points` /
    /// `unity_points1` / `unity_points2` (vendor/server/src/map/packets/s2c/0x061_clistatus.h).
    #[serde(default)]
    pub mjob_no: u8,
    #[serde(default)]
    pub unity_faction: u8,
    #[serde(default)]
    pub unity_points: u32,
    #[serde(default)]
    pub unity_points1: u16,
    #[serde(default)]
    pub unity_points2: u16,
    /// CLISTATUS `mjob_lv`.
    #[serde(default)]
    pub mjob_lv: u8,
    /// CLISTATUS `designation` (title id), `rank`/`rankbar` (conquest rank and its bar),
    /// `BindZoneNo` (home-point zone) and `nation` — the allegiance block of the Status Profile
    /// window, all from the same packet.
    #[serde(default)]
    pub title_id: u16,
    #[serde(default)]
    pub conquest_rank: u16,
    #[serde(default)]
    pub conquest_rank_bar: u16,
    #[serde(default)]
    pub home_point_zone: u16,
    /// 0 San d'Oria, 1 Bastok, 2 Windurst.
    #[serde(default)]
    pub nation: u8,
}

/// s2c 0x00A myroom cluster; present only while inside a Mog House. `model`
/// is an interior model id, not a zone id
/// (vendor/server/src/map/packets/s2c/0x00a_login.cpp GetMogHouseModelID).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyRoomInfo {
    pub model: u16,
    pub sub_map: u8,
    pub exit_bit: u8,
    /// Client-owned "the door is open to visitors" byte. Retail reads/writes game-state
    /// `+0x40C40` (getter FFXiMain.dll retail-2026-09 `.text 0xF96E0`, sole writer `.text 0xF9740`
    /// from the s2c 0x067 entity update); LSB implements neither MYROOM_IS Kind and ships no 0x067,
    /// so this client keeps the byte. Every fresh myroom cluster rebuilds it as closed, which is
    /// exactly LSB's stated reset law (zoning / floor change)
    /// (vendor/server/src/map/packets/c2s/0x0cb_myroom_is.cpp).
    #[serde(default)]
    pub door_open: bool,
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
    /// The packet tail past the dancer block: the ranks behind the Status Profile window's
    /// `Mastery Rank:` row and `Mentor:` marker, plus the thumbs-up permission bit.
    #[serde(default)]
    pub can_thumbs_up_mentor: bool,
    #[serde(default)]
    pub mentor_rank: u8,
    #[serde(default)]
    pub mastery_rank: u8,
}

impl From<ffxi_proto::decode::JobInfo> for JobInfoState {
    fn from(j: ffxi_proto::decode::JobInfo) -> Self {
        Self {
            mjob_no: j.mjob_no,
            sjob_no: j.sjob_no,
            unlocked: j.unlocked,
            sub_job_unlocked: j.sub_job_unlocked,
            job_levels: j.job_levels,
            can_thumbs_up_mentor: j.can_thumbs_up_mentor,
            mentor_rank: j.mentor_rank,
            mastery_rank: j.mastery_rank,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    pub stage: Stage,
    pub account_id: Option<u32>,
    pub char_id: Option<u32>,
    pub self_pet_targid: Option<u16>,
    pub character: Option<String>,
    pub zone_id: Option<u16>,
    pub entities: Vec<Entity>,

    /// Wire-id → index into [`Self::entities`]. Maintained in lockstep by every
    /// `apply_event` arm that mutates the Vec, so self/lookup paths are O(1)
    /// instead of O(N) scans. Transient: not serialized (nothing deserializes
    /// `SessionState` today); rebuilt from scratch on zone change, and after a
    /// single removal, since `retain` shifts every index after the removed slot.
    #[serde(skip)]
    pub entity_index: HashMap<u32, usize>,

    /// Entity ids whose records changed since the last drain of this set.
    /// Stamped by the same `apply_event` arms that mutate [`Self::entities`] —
    /// only when something actually changed — and drained by the translator to
    /// build O(changed) scene deltas instead of full snapshots. A zone change
    /// clears it (the repopulating upserts stamp back in). Transient: not
    /// serialized.
    #[serde(skip)]
    pub pending_entity_upserts: HashSet<u32>,

    /// Entity ids removed since the last drain, plus every live id at a zone
    /// change (see `pending_entity_upserts`). An id present in both sets was
    /// upserted then removed within one batch: removal wins — the translator
    /// must not emit it. Transient: not serialized.
    #[serde(skip)]
    pub pending_entity_removals: HashSet<u32>,

    pub party: Vec<PartyMember>,

    /// An incoming party/alliance invite (s2c 0x0DC GROUP_SOLICIT_REQ) not yet
    /// answered with c2s GROUP_SOLICIT_RES (0x074). The Party-row helper in the
    /// retail client reads exactly this pending state to open `prtyjoin` (YES/NO)
    /// instead of a roster window — FFXiMain.dll retail-2026-09 RVA 0x2016E0,
    /// vault main_menu.md section 24.
    #[serde(default)]
    pub party_invite: Option<PartyInvite>,

    /// Monotonically increasing counter, bumped on every `ZoneChanged`. The
    /// renderer's party-frame content key includes this so a zone transition
    /// forces a UI rebuild, even when the party data looks identical.
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

    /// Newest 0x00A LOGIN `PlayTime` and its arrival instant (retail's pair —
    /// see [`PlayTimeBase`]). Not cleared on a zone change: retail keeps the last
    /// value it was given, and every zone-in replaces it anyway.
    #[serde(default)]
    pub play_time_base: Option<PlayTimeBase>,

    /// The newest s2c 0x062 CLISTATUS2 skill block — the only source for the Status
    /// window's Combat/Magic/Craft rows and for enabling those rows at all.
    #[serde(default)]
    pub char_skills: Option<kuluu_snapshot::CharSkills>,

    /// The newest s2c 0x063 MISCDATA **Merits** record — gates the Status Merit Points row and
    /// feeds its readout (bead kuluu-o9qt7.43; retail's store law at FFXiMain.dll retail-2026-09
    /// .data 0x485826/0x48582A + flag bit1, dispatcher `.text 0x99270`).
    #[serde(default)]
    pub merits: Option<kuluu_snapshot::MisdataMerits>,

    /// The Unity ranking tables (s2c 0x063 type 7) and the s2c 0x110 request mark
    /// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    #[serde(default)]
    pub unity: kuluu_snapshot::UnityState,

    /// The last s2c 0x08E alter ego points block; `None` until one arrives this session.
    #[serde(default)]
    pub alter_ego_points: Option<kuluu_snapshot::AlterEgoPoints>,

    /// An s2c 0x119 has arrived this session (the Status data is ready); never cleared.
    #[serde(default)]
    pub status_data_ready: bool,

    /// s2c 0x00A `LoginState` and `IsMonstrosity` at the last zone-in
    /// (vendor/server/src/map/packets/s2c/0x00a_login.h).
    #[serde(default)]
    pub login_state: u32,
    #[serde(default)]
    pub login_is_monstrosity: bool,

    /// The merits s2c 0x08C stored, keyed by merit index, the job point type state s2c 0x08D
    /// stored by type id, whether any s2c 0x08D arrived this session, and the last s2c 0x063
    /// JobPoints record (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
    #[serde(default)]
    pub merit_entries: std::collections::BTreeMap<u16, kuluu_snapshot::MeritEntry>,
    #[serde(default)]
    pub job_point_types: std::collections::BTreeMap<u16, kuluu_snapshot::JobPointType>,
    #[serde(default)]
    pub job_points_received: bool,
    #[serde(default)]
    pub job_points: Option<kuluu_snapshot::JobPointsRecord>,

    /// The last s2c 0x063 MISCDATA subtype Homepoints record — retail stores its 64-byte mask
    /// block at .data FFXiMain.dll retail-2026-09 0x4858C4 and raises the received flag there
    /// (dispatcher `.text 0x99270`, arm `.text 0x99330`; vault main_menu.md §59 addendum 37).
    #[serde(default)]
    pub homepoints: Option<kuluu_snapshot::HomepointMasks>,

    /// Server-offered alternative to returning home while dead (s2c 0x0F9,
    /// vendor/server/src/map/packets/s2c/0x0f9_res.cpp).
    /// `None` is the ordinary home-point-only menu.
    #[serde(default)]
    pub death_menu_offer: Option<ffxi_proto::decode::DeathMenuOffer>,

    /// The live player trade (s2c 0x021/0x022/0x023/0x025 + our own send/open
    /// transitions; vendor/server/src/map/items/transactions/player_trade.cpp,
    /// vault main_menu.md §25). One at a time — LSB allows exactly one active
    /// transaction per player.
    #[serde(default)]
    pub trade: Option<Box<kuluu_snapshot::TradeWindow>>,

    /// Last completed player search (the `scresult` model singleton in retail;
    /// vault main_menu.md §26). Held until the next query replaces it — retail
    /// keeps the rows visible while you edit the form.
    #[serde(default)]
    pub search: Option<Box<kuluu_snapshot::SearchResults>>,

    /// Both link-shell slots as s2c 0x0E0 last reported them (the `link5`/`lscswin` family's data;
    /// vault main_menu.md §27). LSB pushes both slots at login and again after every
    /// equip/unequip/create, so they are never unknown for long once we are in world.
    #[serde(default)]
    pub linkshells: kuluu_snapshot::LinkShells,

    /// The last s2c 0x06F answer to our synthesis ask (vault main_menu.md §28).
    #[serde(default)]
    pub synth_result: Option<kuluu_snapshot::SynthOutcome>,
    /// The last s2c 0x056 battery fold (vault main_menu.md §30).
    pub missions: kuluu_snapshot::Missions,

    /// Automaton record from the newest s2c 0x044 EXTENDED_JOB PUP frame — the
    /// pupequip Equipment-row variant's whole data source (vault main_menu.md
    /// §39; vendor/server/src/map/utils/charutils.cpp SendExtendedJobPackets).
    #[serde(default)]
    pub automaton: Option<Box<kuluu_snapshot::AutomatonInfo>>,

    /// Monstrosity record from the newest s2c 0x044 EXTENDED_JOB MON frame —
    /// present exactly while in a monstrosity session, kuluu's stand for the
    /// DLL Monstrosity predicate on the Equipment row (vault main_menu.md §39).
    #[serde(default)]
    pub monstrosity: Option<kuluu_snapshot::MonstrosityInfo>,

    /// The client-local macro store behind the mcrmenu family — retail keeps no
    /// server copy (vault main_menu.md §34 wire law) and kuluu's capacity comes
    /// from the DAT grids: 2 blocks x 20 slots (vault main_menu.md §41). Filled
    /// slots only; the editor that mutates this is still open-map on bead .19.1.
    #[serde(default)]
    pub macros: Vec<kuluu_snapshot::MacroSlot>,

    /// The player's arranged emote display order (bead kuluu-o9qt7.34) — the
    /// client-local `emote_order.json` blob for this character, loaded at world
    /// entry and rewritten by the swap law when `abisortw` drives it. Never on
    /// the wire (LSB ships no emote-order packet [V-negative]); retail reads its
    /// own user file at fill time FFXiMain.dll retail-2026-09 `.text 0x2259C6`.
    #[serde(default)]
    pub emote_order: Vec<u8>,

    /// Raw bodies of the last s2c 0x113/0x118 currency reports (bead kuluu-o9qt7.35) — kept
    /// undecoded so the client getter law in `kuluu_render::hud::currency_law` reads them the
    /// way FFXiMain.dll does (vault main_menu.md §59 addendum 27). Nothing is cached beyond
    /// these: every Currencies-window open re-sends the header-only c2s request, and each
    /// reply replaces its block wholesale (`.text 0x99640`/`.text 0x99660` one-shot fold).
    #[serde(default)]
    pub currencies_1: Option<Vec<u8>>,
    #[serde(default)]
    pub currencies_2: Option<Vec<u8>>,

    /// The four SAVE_CONF bitfield bytes from the newest s2c 0x0B4 CONFIG
    /// (vendor/server/src/common/mmo.h; vault main_menu.md §42). LSB echoes a
    /// fresh copy after every c2s CONFIG, so optimistic kuluu toggles reconcile
    /// without extra bookkeeping.
    #[serde(default)]
    pub config_confdata: Option<[u8; 4]>,

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

    /// The server's animation byte for self. 0x037 CHAR_STATUS carries it
    /// directly; the engage edge arrives as the 0x058 battle-target push, because
    /// the server never sends its own 0x0E update (zone_entities.cpp
    /// UpdateEntityPacket skips the entity's own player). Authority for our own
    /// combat stance and rest state.
    #[serde(default)]
    pub self_server_status: u8,

    /// 0x037's `mount_id`, paired with `self_server_status`: which mount we are on,
    /// while that byte says we are on one at all.
    #[serde(default)]
    pub self_mount_id: u8,

    /// Latched when a cutscene's 0x7E mount cue arms the local player's mount
    /// (`AgentEvent::CsMountArmed`): the cue is a client-side write the server
    /// never sees. The override lasts only through the event or until the
    /// server confirms the mount (vendor/server/scripts/effects/mounted.lua).
    #[serde(default)]
    pub cs_mount_armed: bool,

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

    /// Which Mog House arrangement mode this client is in — the analogue of the myroom manager's own mode
    /// word (+0x1C) that the retail mode switch reads back (FFXiMain.dll retail-2026-09 `.text 0x1D34FA`).
    /// Values are the `MODE_*` constants of [`crate::myroom_layout`]; 0 means nothing is being arranged.
    /// The session actor owns the decision and mirrors every accepted change through
    /// [`AgentEvent::MyroomModeChanged`], so this fold copy cannot drift from it.
    #[serde(default)]
    pub myroom_mode: u8,

    /// `SubMapNumber` the server reported in 0x00A LOGIN
    /// (`PChar->loc.boundary`): which sub-area interior the character is
    /// standing in on arrival. Cleared by a zone change, so it only ever
    /// describes the zone currently loaded.
    #[serde(default)]
    pub sub_area: Option<u16>,
    #[serde(default)]
    pub voyage: Option<kuluu_snapshot::Voyage>,

    #[serde(default)]
    pub mog_zone_flag: bool,

    #[serde(default)]
    pub job_info: Option<JobInfoState>,

    /// 2F-unlock bit from the self 0x067 CharSync
    /// (vendor/server/src/map/packets/char_sync.cpp CCharSyncPacket::CCharSyncPacket); `None` until one lands.
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
/// (vendor/server/src/map/packets/c2s/0x106_bazaar_buy.cpp GP_CLI_COMMAND_BAZAAR_BUY::process).
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
/// (job/range/floor — vendor/server/src/map/zone_entities.cpp CZoneEntities::WideScan WideScan).
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
/// (vendor/server/src/map/packets/c2s/0x0dd_equip_inspect.cpp GP_CLI_COMMAND_EQUIP_INSPECT::process).
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

/// Whole milliseconds in a second — the divisor retail's `/playtime` sum uses.
pub const MS_PER_SECOND: u64 = 1_000;

/// The play-time pair retail keeps for the character: the `PlayTime` seconds the
/// server last reported, and when that report arrived. Retail stores exactly this
/// pair — the packet's u32 plus a clock stamp fetched immediately after it
/// (FFXiMain.dll retail-2026-09 RVA 0xFB3EE reads `[packet+0xA0]` into
/// `[state+0x2d67c]`, then `.text 0xFB3FA` stores the `call 0x1039E0` clock next to it)
/// and sums base + elapsed at `/playtime` (handler RVA 0x7CAB0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PlayTimeBase {
    /// `PlayTime` — seconds counted by the server as of that zone-in.
    pub base_secs: u32,

    /// [`process_monotonic_ms`] reading at the moment that 0x00A arrived.
    pub arrival_ms: u64,
}

impl PlayTimeBase {
    /// Base plus whole seconds elapsed since arrival, exactly as retail sums it
    /// (FFXiMain.dll retail-2026-09 RVA 0x7CAB0 calls the clock helper
    /// `.text 0xFC770` and adds the stored base).
    pub fn seconds_at(&self, now_ms: u64) -> u64 {
        let elapsed = now_ms.saturating_sub(self.arrival_ms) / MS_PER_SECOND;
        u64::from(self.base_secs).saturating_add(elapsed)
    }
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
    /// Whether ESC may cancel this event (retail's CliEventCancelFlag; the VM's
    /// 0x42 disarms it in cutscenes that lock you in, 0x2E re-arms). Defaults to
    /// true so frames from an unknown producer stay cancellable. A customMenu
    /// frame keeps it armed so ESC answers "Canceled." through the customMenu
    /// branch.
    #[serde(default = "cancel_armed_default")]
    pub cancel_armed: bool,
    /// The speaking entity's target index for this frame; `None` is a line the
    /// bytecode prints with no speaker (retail renders those headerless).
    #[serde(default)]
    pub speaker_index: Option<u16>,
    /// The line carried an item / key-item marker (`{Item:N}` / `{KeyItem:N}`)
    /// before substitution: enternity-style auto-advance leaves such lines
    /// manual (the addon's "sentences that contain items will not be skipped").
    #[serde(default)]
    pub contains_item: bool,
    /// Seconds the retail message box holds this frame before dismissing it on
    /// its own — the entry's `7F 34/35/36 NN` auto-prompt code
    /// (research/cexi-docs/dialog/format.md); `None` waits for a key press.
    /// The session counts it down and advances exactly like a key press.
    #[serde(default)]
    pub auto_advance: Option<u8>,
}

fn cancel_armed_default() -> bool {
    true
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// 0x6C TRANSPAR: fade `target`'s alpha to `end_alpha` (0..=255) over
    /// `duration_frames` frames (research/XiEvents/OpCodes/0x006C.md).
    Transpar {
        target: CutsceneActor,
        end_alpha: i32,
        duration_frames: i32,
    },
    CameraLock {
        lock: bool,
    },
    /// The decoded 0x38 mode word; visibility policy is unresolved.
    LocalMode {
        mode: u16,
    },
    /// 0x20: write retail's `CliEventUcFlag`; while it holds, the player's
    /// `CanIMove` is false (research/XiEvents/OpCodes/0x0020.md,
    /// research/XIClient ActorTelemetry::CanIMove).
    PlayerControl {
        locked: bool,
    },
    /// 0x67/0x68 HIDE_HUD/SHOW_HUD: hide or show the entire HUD UI for the
    /// rest of the cutscene (research/XiEvents/OpCodes/0x0067.md, 0x0068.md).
    HudHide {
        hide: bool,
    },
    /// 0x77/0x78/0xA9/0xC9 game-clock holds: hold the clock at Vana'diel hour
    /// `hour`, minute `minute`, on Vana day `day_from_epoch` from the calendar
    /// epoch when set (else the current day), or release it back to server
    /// time (research/XiEvents/OpCodes/0x0077.md, 0x0078.md, 0x00A9.md,
    /// 0x00C9.md).
    ClockHold {
        stop: bool,
        hour: Option<u32>,
        #[serde(default)]
        minute: u8,
        #[serde(default)]
        day_from_epoch: Option<u32>,
    },
    Mount {
        target: CutsceneActor,
        status_event: u8,
        mount_id: Option<u16>,
    },
    ExtScheduler {
        motion: Option<kuluu_snapshot::ExtSchedulerMotion>,
        actor: CutsceneActor,
        partner: CutsceneActor,
        key: ffxi_event::FourCc,
    },
    /// Start zone-level scheduler routine `key` over the two actors (the
    /// 0x2D/0x54 pair, research/XiEvents/OpCodes/0x002D.md); the host resolves
    /// `key` out of the current zone's own model DAT (`zone_id`).
    ZoneScheduler {
        key: ffxi_event::FourCc,
        actor: CutsceneActor,
        partner: CutsceneActor,
        zone_id: u16,
    },
    /// Walk `actor` to `(x, y, z)` at `speed`, facing `heading`; the
    /// coordinates are the VM's event-coordinate integers.
    ActorMove {
        actor: CutsceneActor,
        x: i32,
        y: i32,
        z: i32,
        heading: i32,
        /// Raw 0x32 MainSpeed operand (research/XiEvents/OpCodes/0x0032.md);
        /// the renderer scales it with `ffxi_event::vm::scene::EVENT_SPEED_SCALE`.
        speed: i32,
    },
    /// Snap `actor` to `(x, y, z)` facing `heading`, in event-coordinate
    /// integers.
    ActorPlace {
        actor: CutsceneActor,
        x: i32,
        y: i32,
        z: i32,
        heading: i32,
    },
    /// Face `actor` toward `heading`, in the VM's 4096-step full-circle units.
    ActorFace {
        actor: CutsceneActor,
        heading: i32,
    },
    /// Turn `actor` to face `target`.
    ActorLookAt {
        actor: CutsceneActor,
        target: CutsceneActor,
    },
    /// Stop the current action; `key` supplies the replacement idle motion.
    ActorStopAction {
        actor: CutsceneActor,
        key: Option<ffxi_event::FourCc>,
    },
    /// 0xB5 case 0 (research/XiEvents/OpCodes/0x00B5.md): set `actor`'s display
    /// name to `name` (the event's work string, filled from an inline literal
    /// or the s2c 0x005D PENDINGSTR table,
    /// research/XiPackets/world/server/0x005D). The nameplate re-rasters from
    /// it until the event ends.
    EntityName {
        actor: CutsceneActor,
        name: [u8; 16],
    },
}

/// Number of music slots [`AgentEvent::MusicVolumeChanged::slot`] can name
/// (vendor/server/data/enums/music_slot.yaml `MusicSlot`, ZoneDay..Fishing).
/// The 0x5D event opcode sets retail's single master music volume, so it is
/// carried as the same volume on every slot.
pub const MUSIC_SLOT_COUNT: u8 = 8;

/// Wares an NPC shop can hold. The retail client's shop table is a fixed
/// 80-entry array (research/XIClient GC_SHOP_SYS `Entries`), and the server
/// fills it 19 rows at a time.
pub const SHOP_TABLE_CAPACITY: usize = 80;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShopState {
    /// `ShopItemOffsetIndex` of the most recent s2c 0x03C
    /// (vendor/server/src/map/packets/s2c/0x03c_shop_list.cpp). Rows accumulate
    /// at that offset rather than replacing the table, so a shop wider than
    /// one packet lists in full.
    pub offset_index: u16,
    pub items: Vec<ShopItem>,
    /// s2c 0x03E SHOP_OPEN (vendor/server/src/map/packets/s2c/0x03e_shop_open.cpp)
    /// arrived: the window is up even before any row does.
    pub opened: bool,

    /// `ShopListNum` from s2c 0x03E (vendor/server/src/map/packets/s2c/0x03e_shop_open.cpp)
    /// — how many rows the server intends to send.
    #[serde(default)]
    pub expected_items: u16,

    /// The final s2c 0x03C (Flags bit 0,
    /// vendor/server/src/map/packets/s2c/0x03c_shop_list.cpp) has landed, so
    /// `items` is the whole stock.
    #[serde(default)]
    pub complete: bool,

    /// The NPC this shop belongs to — the entity the client last sent an
    /// `ActionKind::Talk` at. 0 when nothing was resolved (a shop opened from a
    /// server-driven menu rather than a trigger). The window closes when this
    /// entity leaves range or the zone.
    #[serde(default)]
    pub vendor_id: u32,

    /// The appraisal a SHOP_SELL_REQ came back with (s2c 0x03D,
    /// vendor/server/src/map/packets/s2c/0x03d_shop_sell.cpp), awaiting the
    /// player's yes/no before the SHOP_SELL_SET that completes the sale.
    #[serde(default)]
    pub pending_sale: Option<ShopSale>,
}

/// A sale the server has priced but the player has not yet confirmed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShopSale {
    /// LOC_INVENTORY slot being sold (`PropertyItemIndex`).
    pub item_index: u8,
    pub item_no: u16,
    /// Per-unit price the server appraised the item at.
    pub unit_price: u32,
    pub count: u32,
}

impl ShopSale {
    pub fn total_gil(&self) -> u32 {
        self.unit_price.saturating_mul(self.count)
    }
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
    /// The server has the slot marked unpickable — see
    /// [`ffxi_proto::decode::lock_flg::NO_SELECT`].
    #[serde(default)]
    pub unselectable: bool,
    pub price: u32,
    #[serde(default)]
    pub charges_remaining: Option<u8>,
    #[serde(default)]
    pub next_use_vana_ts: Option<u32>,
    #[serde(default)]
    pub use_delay_end_vana_ts: Option<u32>,
    #[serde(default)]
    pub ready: Option<bool>,

    /// The record's 24-byte Attr, verbatim as s2c 0x020 carried it — retail keeps the whole thing on
    /// its inventory record (the record is 44 bytes with Attr at `+0x10`, FFXiMain.dll retail-2026-09
    /// `.text 0xEC630`) because several unrelated subsystems read different parts of it: charged
    /// items above, and the Mog House furniture layer/pose bytes
    /// (see [`crate::myroom_furniture`]). `None` when no Attr record has been seen for this slot yet.
    #[serde(default)]
    pub extdata: Option<[u8; 24]>,
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

/// GP_CLI_COMMAND_PBX_BOXNO (vendor/server/src/map/packets/c2s/0x04d_pbx.h).
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

    /// The Attack has been sent and the server has not answered. Readers that
    /// mean "the server has us engaged" match `Engaged` only; this variant
    /// exists so nothing reads as engaged on a refusal.
    Engaging {
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

/// Pending invite carried from s2c 0x0DC
/// (vendor/server/src/map/packets/s2c/0x0dc_group_solicit_req.h) to the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyInvite {
    /// Inviter's char id (`UniqueNo`).
    pub char_id: u32,
    /// Inviter's display name (`sName`).
    pub name: String,
    /// Kind == PartyKind::Alliance (5) rather than Party (0).
    #[serde(default)]
    pub alliance: bool,
}

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
    /// "no party"). vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp GP_SERV_COMMAND_GROUP_LIST::GP_SERV_COMMAND_GROUP_LIST.
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

    /// Not the party `MoghouseFlg` byte: that is a per-member display flag LSB
    /// leaves unpopulated and forks fill in their own way
    /// (vendor/server/src/map/packets/s2c/0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN).
    pub fn self_in_mog_house(&self) -> bool {
        self.myroom.is_some()
    }

    /// Riding is read off the broadcast animation byte, not `self_mount_id`:
    /// 0x037 carries the mount *identity* but the animation byte is what says
    /// we are on it, and it is the field every observer sees too.
    pub fn self_mounted(&self) -> bool {
        ffxi_proto::decode::animation::is_mounted(self.self_server_status)
    }

    /// Play time to report now: the stored base plus seconds elapsed since that
    /// 0x00A arrived, or 0 while no LOGIN has carried `PlayTime`.
    pub fn play_time_seconds(&self) -> u64 {
        self.play_time_base
            .map(|b| b.seconds_at(process_monotonic_ms()))
            .unwrap_or(0)
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
}

/// One drained batch of [`SessionState::pending_entity_upserts`] /
/// [`SessionState::pending_entity_removals`]. The event folder drains it after
/// each fold and forwards it to the translator, which merges batches into
/// O(changed) scene deltas. `other_changed` marks a batch whose triggering
/// event also mutated non-entity state (chat, party, stage, …); the translator
/// answers those with a full snapshot instead of an entity-only delta.
#[derive(Debug, Clone, Default)]
pub struct EntityChanges {
    pub upserts: HashSet<u32>,
    pub removals: HashSet<u32>,
    pub other_changed: bool,
}

impl SessionState {
    /// Takes both pending entity sets (leaving them empty). The event folder
    /// drains this after each fold; ids present in `removals` win over
    /// `upserts` — an upsert-then-remove within one batch nets to a removal.
    pub fn take_pending_entities(&mut self) -> (HashSet<u32>, HashSet<u32>) {
        (
            std::mem::take(&mut self.pending_entity_upserts),
            std::mem::take(&mut self.pending_entity_removals),
        )
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
            // The client-local macro store (bead kuluu-o9qt7.19.2): nothing on
            // the wire ever carries macros (vault main_menu.md §41), so this is
            // the only way they enter folded state.
            AgentEvent::MacrosLoaded { slots } => {
                self.macros = slots.clone();
                true
            }
            // The emote display order enters the same way (bead kuluu-o9qt7.34):
            // loaded at world entry or echoed after a commit, wholesale.
            AgentEvent::EmoteOrderLoaded { ids } => {
                self.emote_order = ids.clone();
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
                self.voyage = None;

                self.logout_countdown = None;
                self.death_homepoint_secs = None;
                self.death_menu_offer = None;

                // Every live id is gone (vendor/server/src/map/packets/s2c/0x008_enterzone.cpp):
                // stamp them all as removals so a delta drained before the
                // repopulating upserts still sees the wipe, and drop pending
                // upserts (they belong to the zone being left). The index dies
                // with the Vec.
                self.pending_entity_removals = self.entities.iter().map(|e| e.id).collect();
                self.pending_entity_upserts.clear();
                self.entity_index.clear();
                self.entities.clear();
                self.party.clear();
                self.zone_generation = self.zone_generation.wrapping_add(1);

                self.current_weather = None;
                self.check_result = None;
                self.check_message = None;
                // The seller is a zone-local entity, so a bazaar cannot survive
                // the warp: LSB resolves the browsed bazaar through
                // `GetEntity(BazaarID.targid)` and drops any request once that
                // lookup fails (0x106_bazaar_buy.cpp GP_CLI_COMMAND_BAZAAR_BUY::process).
                self.bazaar = None;
                // The AH counter is likewise zone-local (sendMenu from the NPC;
                // GP_CLI_COMMAND_AUC::validate gates on the zone's MISC_AH).
                self.auction = AuctionState::default();
                self.self_casting = None;
                self.self_server_status = 0;
                self.cs_mount_armed = false;

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
            AgentEvent::VoyageSynced { voyage } => {
                self.voyage = Some(*voyage);
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
                    if let Some(idx) = self.entity_index.get(&char_id).copied() {
                        let ent = &mut self.entities[idx];
                        changed = ent.pos != pos.pos
                            || ent.heading != pos.heading
                            || ent.speed != pos.speed
                            || ent.speed_base != pos.speed_base;
                        if changed {
                            ent.pos = pos.pos;
                            ent.heading = pos.heading;
                            ent.speed = pos.speed;
                            ent.speed_base = pos.speed_base;
                            self.pending_entity_upserts.insert(char_id);
                        }
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
                if let Some(idx) = self.entity_index.get(&entity.id).copied() {
                    let existing = &mut self.entities[idx];
                    let preserved_name = entity.name.clone().or_else(|| existing.name.clone());
                    let merged_kind = merge_kind(existing.kind, entity.kind);

                    let preserved_hp_pct = entity.hp_pct.or(existing.hp_pct);

                    let preserved_look = entity.look.or(existing.look).or(latched_self_look);
                    let preserved_npc_state = entity.npc_state.or(existing.npc_state);
                    // Flags4.JobMasterFlag refreshes on every non-despawn 0x0D
                    // (char_update.cpp), unlike the General words — fold the
                    // freshest value into the preserved flags so a pos-only tick
                    // still carries it.
                    let base_char_flags = entity.char_flags.or(existing.char_flags);
                    let preserved_char_flags = match (base_char_flags, entity.job_master_display) {
                        (Some(mut flags), Some(job_master)) => {
                            flags.job_master_display = job_master;
                            Some(flags)
                        }
                        // No General update has arrived yet — a spawn carries
                        // one, so this is defensive (char_update.cpp).
                        // Materialize the flags with just the star.
                        (None, Some(true)) => Some(ffxi_proto::decode::CharFlags {
                            job_master_display: true,
                            ..Default::default()
                        }),
                        _ => base_char_flags,
                    };
                    let preserved_mount_id = entity.mount_id.or(existing.mount_id);
                    // Model-block-gated at the source (char_update.cpp), so merge
                    // like mount_id — never off pos_present.
                    let preserved_monstrosity = entity.monstrosity.or(existing.monstrosity);
                    // UPDATE_HP-gated at the source (entity_update.cpp CEntityUpdatePacket::updateWith), so
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
                        monstrosity: preserved_monstrosity,
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
                        self.pending_entity_upserts.insert(entity.id);
                        true
                    }
                } else {
                    let mut inserted = entity.clone();
                    inserted.look = inserted.look.or(latched_self_look);
                    self.entity_index.insert(entity.id, self.entities.len());
                    self.entities.push(inserted);
                    self.pending_entity_upserts.insert(entity.id);
                    self.pending_entity_removals.remove(&entity.id);
                    true
                }
            }
            AgentEvent::EntityRemoved { id } => {
                let before = self.entities.len();
                self.entities.retain(|e| e.id != *id);
                if self.entities.len() != before {
                    self.entity_index = self
                        .entities
                        .iter()
                        .enumerate()
                        .map(|(i, e)| (e.id, i))
                        .collect();
                    self.pending_entity_upserts.remove(id);
                    self.pending_entity_removals.insert(*id);
                }
                self.entities.len() != before
            }
            AgentEvent::OwnPetSynced { targid } => {
                let changed = self.self_pet_targid != *targid;
                self.self_pet_targid = *targid;
                changed
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
                allegiance,
            } => {
                let idx = match (id, act_index) {
                    (Some(wire_id), _) => self.entity_index.get(wire_id).copied(),
                    (None, Some(act)) => self.entities.iter().position(|e| e.act_index == *act),
                    _ => None,
                };
                let existing = idx.and_then(|i| self.entities.get_mut(i));
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
                    if let Some(a) = allegiance {
                        // Self's entity may still carry no flags at all (it does not
                        // receive its own 0x0D, research/XiPackets/world/server/0x000D),
                        // so materialize rather than skip.
                        if existing.char_flags.as_ref().map(|f| f.allegiance) != Some(*a) {
                            let flags = existing.char_flags.get_or_insert_with(Default::default);
                            flags.allegiance = *a;
                            changed = true;
                        }
                    }
                    if changed {
                        self.pending_entity_upserts.insert(existing.id);
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
                    || self.voyage.is_some()
                    || self.widescan != WidescanList::default()
                    || self.cs_mount_armed;
                self.cs_mount_armed = false;
                self.stage = Stage::Disconnected;
                self.diagnostics.stage = Some(Stage::Disconnected);

                self.logout_countdown = None;
                self.voyage = None;
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
            AgentEvent::PartySolicitReceived {
                char_id,
                name,
                alliance,
            } => {
                // s2c 0x0DC: retail parks the invite on static party state and
                // (re)opens `prtyjoin` when its Party-row helper runs; we stamp
                // it here so the snapshot carries it the same way `party` does.
                let next = Some(PartyInvite {
                    char_id: *char_id,
                    name: name.clone(),
                    alliance: *alliance,
                });
                let changed = self.party_invite != next;
                self.party_invite = next;
                changed
            }
            AgentEvent::PartySolicitWithdrawn => {
                // s2c 0x0DE GROUP_SOLICIT_NO: drop the pending invite (the retail
                // helper's `alarm`-close pairing, vault main_menu.md section 24).
                let had = self.party_invite.is_some();
                self.party_invite = None;
                had
            }
            AgentEvent::TradeIncoming { char_id } => {
                // s2c 0x021: we are the invited side. Retail answers through its
                // own trade window's rows (vault main_menu.md section 25); ours
                // opens with answer rows via incoming_pending.
                let fresh = kuluu_snapshot::TradeWindow {
                    partner_id: *char_id,
                    incoming_pending: true,
                    active: false,
                    my_slots: Vec::new(),
                    partner_slots: Vec::new(),
                    my_confirmed: false,
                    partner_confirmed: false,
                };
                let next = Some(Box::new(fresh));
                let changed = self.trade != next;
                self.trade = next;
                changed
            }
            AgentEvent::TradeOutgoingOpened { char_id } => {
                // Our own TRADE_REQ went out: retail opens its windows on send
                // (helper 0x13FE70's success path, vault main_menu.md section 25).
                if self
                    .trade
                    .as_ref()
                    .is_some_and(|w| w.partner_id == *char_id && !w.incoming_pending)
                {
                    return false;
                }
                self.trade = Some(Box::new(kuluu_snapshot::TradeWindow::outgoing(*char_id)));
                true
            }
            AgentEvent::TradeRes { kind } => {
                // s2c 0x022: Start opens both windows once the target accepted;
                // every terminal kind (Cancell/End/Err*) closes ours — LSB keeps
                // at most one transaction per player, so no partner-matching is
                // needed to clear it safely.
                use ffxi_proto::decode::TradeResKind as K;
                match kind {
                    // Start means the transaction exists now, whichever phase our
                    // window was in (invited-and-accepted, or our own request).
                    K::Start => match self.trade.as_mut() {
                        Some(w) => {
                            let already = w.active && !w.incoming_pending;
                            w.incoming_pending = false;
                            w.active = true;
                            !already
                        }
                        None => {
                            // A Start with no window — the 0x21/0x32 race we never
                            // saw: open active against the packet's sender.
                            self.trade = Some(Box::new(kuluu_snapshot::TradeWindow {
                                partner_id: 0,
                                incoming_pending: false,
                                active: true,
                                my_slots: Vec::new(),
                                partner_slots: Vec::new(),
                                my_confirmed: false,
                                partner_confirmed: false,
                            }));
                            true
                        }
                    },
                    _ => {
                        let had = self.trade.is_some();
                        self.trade = None;
                        had
                    }
                }
            }
            AgentEvent::TradeMySlotSet { trade_index, offer } => {
                // s2c 0x025 echo of our staging. Any slot mutation also un-confirms
                // both sides (player_trade.cpp setSlot clears `accepted` for both).
                let Some(w) = self.trade.as_mut() else {
                    // With no transaction open the views describe a partner we
                    // already walked away from — drop them (LSB allows exactly
                    // one PlayerTradeTransaction per player).
                    return false;
                };
                if apply_trade_slot(&mut w.my_slots, *trade_index, *offer) {
                    // Any slot mutation un-confirms BOTH sides
                    // (player_trade.cpp setSlot clears `accepted`).
                    w.my_confirmed = false;
                    w.partner_confirmed = false;
                    true
                } else {
                    false
                }
            }
            AgentEvent::TradePartnerSlotSet { trade_index, offer } => {
                let Some(w) = self.trade.as_mut() else {
                    return false;
                };
                if apply_trade_slot(&mut w.partner_slots, *trade_index, *offer) {
                    // Partner-side views un-confirm us too — the offer changed
                    // under our feet (player_trade.cpp pushSlotView → both
                    // `accepted=false`, vault main_menu.md section 25).
                    w.my_confirmed = false;
                    w.partner_confirmed = false;
                    true
                } else {
                    false
                }
            }
            AgentEvent::TradeMadeLocally => {
                // Retail stamps its confirm state at Make-send time (select id 9
                // sets the window's state word to 4 before any echo, vault
                // main_menu.md section 25); the commit comes as RES End/Cancell.
                if let Some(w) = self.trade.as_mut() {
                    if !w.my_confirmed {
                        w.my_confirmed = true;
                        return true;
                    }
                }
                false
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
                    if let Some(idx) = self.entity_index.get(&char_id).copied() {
                        let ent = &mut self.entities[idx];
                        changed = ent.pos != target.pos || ent.heading != target.heading;
                        if changed {
                            ent.pos = target.pos;
                            ent.heading = target.heading;
                            self.pending_entity_upserts.insert(char_id);
                        }
                    }
                }
                changed
            }
            // 0x058 (GP_SERV_COMMAND_ASSIST) is pushed by
            // vendor/server/src/map/ai/controllers/player_controller.cpp
            // CPlayerController::Engage only after its IsValidTarget,
            // distance and MsgBasic::WaitLonger checks pass and
            // CController::Engage returns true. It is therefore the server's
            // accept, and the synchronous one: 0x037 CHAR_STATUS carries the
            // same byte from entities/char_entity.cpp CCharEntity::PostTick
            // behind m_nextUpdateTimer. A refused engage sends neither. The
            // write is gated on our own pending or standing engage goal
            // because battleutils::assistTarget pushes the same packet for
            // /assist.
            AgentEvent::TargetChanged { target_id } => {
                let engaged = matches!(
                    self.current_goal,
                    Some(
                        ReactorGoalSnapshot::Engaging { .. } | ReactorGoalSnapshot::Engaged { .. }
                    )
                );
                let status = match target_id {
                    Some(_) if engaged => ffxi_proto::decode::animation::ATTACK,
                    Some(_) => return false,
                    None => ffxi_proto::decode::animation::NONE,
                };
                let changed = self.self_server_status != status;
                self.self_server_status = status;
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
            | AgentEvent::EngageRefused { .. }
            | AgentEvent::Knockbacks { .. }
            | AgentEvent::VanaTimeSynced { .. } => false,
            AgentEvent::InventoryUpdated { container, update } => {
                let entry = self.inventory.containers.entry(*container).or_default();
                match update {
                    InventoryUpdate::Capacities { capacities } => {
                        // Zeros apply too: 0 is LSB's "container disabled"
                        // sentinel (e.g. an expired Mog Locker lease across a
                        // zone change) — sticky grants would keep offering a
                        // bag the server rejects (s2c/0x01c_item_max.cpp GP_SERV_COMMAND_ITEM_MAX::GP_SERV_COMMAND_ITEM_MAX).
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
                            // ITEM_LIST carries no extdata, and LSB's zone-in
                            // flood sends one for every equipped item right
                            // after the ITEM_ATTR that stamped its charges and
                            // equip delay (vendor/server/src/map/utils/charutils.cpp
                            // SendInventory); a same-id update without charge
                            // info keeps the timers it cannot restate.
                            // The same rule covers the raw Attr: retail's furniture law reads layer
                            // and pose straight out of it (FFXiMain.dll retail-2026-09 `.text 0x1CCFD1`),
                            // so dropping it on an ITEM_LIST re-fold would empty the object table.
                            let kept_extdata =
                                slot.extdata.is_none() && existing.item_no == slot.item_no;
                            let kept_charges = slot.charges_remaining.is_none()
                                && existing.item_no == slot.item_no;
                            let kept = kept_charges.then_some((
                                existing.charges_remaining,
                                existing.next_use_vana_ts,
                                existing.use_delay_end_vana_ts,
                                existing.ready,
                            ));
                            let previous_extdata = existing.extdata;
                            *existing = slot.clone();
                            if kept_extdata {
                                existing.extdata = previous_extdata;
                            }
                            if let Some((charges, next_use, delay_end, ready)) = kept {
                                existing.charges_remaining = charges;
                                existing.next_use_vana_ts = next_use;
                                existing.use_delay_end_vana_ts = delay_end;
                                existing.ready = ready;
                            }
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
                    // index 0 (charutils.cpp UnequipItem queueEquipChange(LOC_INVENTORY,
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
                    if let Some(idx) = self.entity_index.get(&char_id).copied() {
                        let ent = &mut self.entities[idx];
                        changed |= ent.look != Some(*look);
                        if changed && ent.look != Some(*look) {
                            ent.look = Some(*look);
                            self.pending_entity_upserts.insert(char_id);
                        }
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
            | AgentEvent::MapOpen { .. }
            | AgentEvent::MapMarkerPlaced { .. }
            | AgentEvent::EventSoundChannelsSet { .. }
            | AgentEvent::EventSoundChannelsChange { .. }
            | AgentEvent::EventMapPropsState { .. }
            | AgentEvent::MapReloadRequested { .. }
            | AgentEvent::ActorRenderFlagChanged { .. }
            | AgentEvent::MapClosed => false,
            AgentEvent::CutsceneEnded => {
                let changed = self.cs_mount_armed;
                self.cs_mount_armed = false;
                changed
            }
            AgentEvent::EventDialog { dialog } => {
                let changed = self.dialog.as_ref() != Some(dialog);
                self.dialog = Some(dialog.clone());
                changed
            }
            AgentEvent::DialogDismissed => {
                let changed = self.dialog.is_some();
                self.dialog = None;
                changed
            }
            AgentEvent::ShopUpdated { shop } => {
                let changed = self.shop.as_ref() != Some(shop);
                self.shop = Some(shop.clone());
                changed
            }
            AgentEvent::ShopClosed => {
                let changed = self.shop.is_some();
                self.shop = None;
                changed
            }
            AgentEvent::ShopSellAppraisal { .. } => false,
            AgentEvent::StatusIconsUpdated { icons, expiries } => {
                let changed = self.status_icons != *icons || self.status_icon_expiries != *expiries;
                self.status_icons = icons.clone();
                self.status_icon_expiries = expiries.clone();
                changed
            }
            AgentEvent::AbilityRecastsUpdated { recasts } => {
                // Any s2c 0x119 readies the Status data for the session
                // (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
                let changed = self.ability_recasts != *recasts || !self.status_data_ready;
                self.ability_recasts = recasts.clone();
                self.status_data_ready = true;
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
            AgentEvent::PlayTimeSynced { seconds } => {
                let arrival_ms = process_monotonic_ms();
                let changed = self.play_time_base.map(|b| b.base_secs) != Some(*seconds);
                self.play_time_base = Some(PlayTimeBase {
                    base_secs: *seconds,
                    arrival_ms,
                });
                changed
            }
            AgentEvent::CurrenciesLoaded { mode, body } => {
                // Wire discriminants: 0 = report 1 (s2c 0x113), 1 = report 2 (s2c 0x118) —
                // the typed view lives in `CurrencyMode::from_wire` on the render side. Each
                // arrival replaces its block wholesale; an unknown discriminant drops the
                // packet like any unexpected reply would (the request path never sends one).
                let slot = match mode {
                    0 => &mut self.currencies_1,
                    1 => &mut self.currencies_2,
                    _ => return false,
                };
                let changed = slot.as_deref() != Some(body.as_slice());
                *slot = Some(body.clone());
                changed
            }
            AgentEvent::CharSkillsUpdated { skills } => {
                let changed = self.char_skills != Some(*skills);
                self.char_skills = Some(*skills);
                changed
            }
            AgentEvent::MeritsUpdated { merits } => {
                // Each MISCDATA Merits record replaces the last, mirroring retail's dispatcher
                // storing over 0x485826/0x48582A on every arrival (FFXiMain.dll retail-2026-09
                // `.text 0x99290`).
                let changed = self.merits != Some(*merits);
                self.merits = Some(*merits);
                changed
            }
            AgentEvent::AlterEgoPointsUpdated { points } => {
                let changed = self.alter_ego_points != Some(*points);
                self.alter_ego_points = Some(*points);
                changed
            }
            AgentEvent::UnityRankingChunk {
                result_set,
                data_type,
                concord_values,
            } => fold_unity_ranking_chunk(&mut self.unity, *result_set, *data_type, concord_values),
            AgentEvent::UnityRankingRequested { result_set } => {
                let Some(set) = self
                    .unity
                    .rankings
                    .get_mut(unity_result_set_index(*result_set))
                else {
                    return false;
                };
                std::mem::replace(&mut set.complete, false)
            }
            AgentEvent::UnityQuestUpdated { .. } => {
                !std::mem::replace(&mut self.unity.quest_armed, true)
            }
            AgentEvent::UnityQuestRequested => {
                std::mem::replace(&mut self.unity.quest_armed, false)
            }
            AgentEvent::LoginStateSynced {
                login_state,
                is_monstrosity,
            } => {
                let changed = self.login_state != *login_state
                    || self.login_is_monstrosity != *is_monstrosity;
                self.login_state = *login_state;
                self.login_is_monstrosity = *is_monstrosity;
                changed
            }
            AgentEvent::MeritsListed { entries } => {
                fold_merit_entries(&mut self.merit_entries, entries)
            }
            AgentEvent::JobPointsListed { entries } => {
                let stored = fold_job_point_entries(&mut self.job_point_types, entries);
                !std::mem::replace(&mut self.job_points_received, true) || stored
            }
            AgentEvent::JobPointsRecordUpdated { record } => {
                let changed = self.job_points != Some(*record);
                self.job_points = Some(*record);
                changed
            }
            AgentEvent::HomepointsUpdated { masks } => {
                // Each Homepoints record replaces the last: retail's arm rep-movsds over the whole
                // 64-byte store on every arrival (`.text FFXiMain.dll retail-2026-09 0x99330`).
                let changed = self.homepoints != Some(*masks);
                self.homepoints = Some(*masks);
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
                // The shop is deliberately untouched: it is not event-scoped
                // (a vendor runs `showText` + `sendMenu`, not `startEvent` —
                // vendor/server/scripts/globals/shop.lua), and the server even
                // refuses SHOP_BUY while InEvent. `ShopClosed` owns its close.
                let changed = self.dialog.is_some();
                self.dialog = None;
                changed
            }
            AgentEvent::SelfServerStatus { status, mount_id } => {
                if self.cs_mount_armed && !ffxi_proto::decode::animation::is_mounted(*status) {
                    return false;
                }
                self.cs_mount_armed = false;
                let changed = self.self_server_status != *status || self.self_mount_id != *mount_id;
                self.self_server_status = *status;
                self.self_mount_id = *mount_id;
                changed
            }
            AgentEvent::CsMountArmed { status, mount_id } => {
                let changed = self.self_server_status != *status || self.self_mount_id != *mount_id;
                self.self_server_status = *status;
                self.self_mount_id = *mount_id;
                self.cs_mount_armed = ffxi_proto::decode::animation::is_mounted(*status);
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
            AgentEvent::PlayerSearchCompleted { total, players } => {
                // Whole-set replacement, like the set_search delta law: retail's
                // scresult model is rebuilt from a new query's frames.
                let next = Some(Box::new(kuluu_snapshot::SearchResults {
                    total: *total,
                    players: players.clone(),
                }));
                let changed = self.search != next;
                self.search = next;
                changed
            }

            AgentEvent::LinkShellSlotChanged {
                linkshell_slot,
                category,
                item_index,
            } => {
                // One s2c 0x0E0 describes one slot; the pair is rebuilt whole so the delta stays a
                // last-writer-wins `set_linkshells` (vault main_menu.md §27).
                let Some(slot) = linkshell_slot_for(*linkshell_slot) else {
                    return false;
                };
                let mut next = self.linkshells;
                let empty = *category == 0 && *item_index == 0;
                let value = if empty {
                    kuluu_snapshot::LinkShellSlot::EMPTY
                } else {
                    kuluu_snapshot::LinkShellSlot {
                        category: *category,
                        item_index: *item_index,
                    }
                };
                match slot {
                    1 => next.one = value,
                    _ => next.two = value,
                }
                let changed = next != self.linkshells;
                if changed {
                    self.linkshells = next;
                }
                changed
            }
            AgentEvent::MyroomModeChanged { previous: _, next } => {
                if self.myroom_mode == *next {
                    return false;
                }
                self.myroom_mode = *next;
                true
            }
            AgentEvent::SynthCompleted { outcome } => {
                // Each answer replaces the last whole (s2c 0x06F is one-shot feedback).
                let changed = self.synth_result.as_ref() != Some(outcome);
                if changed {
                    self.synth_result = Some(outcome.clone());
                }
                changed
            }
            AgentEvent::MissionInfoUpdated { info } => {
                // Each main-port frame replaces the mission log whole (vault main_menu.md §30).
                let changed = self.missions.info.as_ref() != Some(info);
                if changed {
                    self.missions.info = Some(*info);
                }
                changed
            }
            AgentEvent::AutomatonUpdated { automaton } => {
                // One s2c 0x044 PUP frame replaces the automaton record whole
                // (vault main_menu.md §39).
                let next = Some(Box::new(automaton.clone()));
                let changed = self.automaton != next;
                if changed {
                    self.automaton = next;
                }
                changed
            }
            AgentEvent::ConfigUpdated { confdata } => {
                // The server restates the whole SAVE_CONF prefix, so the newest
                // frame replaces it wholesale (vault main_menu.md §42).
                let changed = self.config_confdata != Some(*confdata);
                if changed {
                    self.config_confdata = Some(*confdata);
                }
                changed
            }
            AgentEvent::MonstrosityUpdated { monstrosity } => {
                // One s2c 0x044 MON frame replaces the record whole (vault
                // main_menu.md §39).
                let changed = self.monstrosity.as_ref() != Some(monstrosity);
                if changed {
                    self.monstrosity = Some(*monstrosity);
                }
                changed
            }
            AgentEvent::MissionTvrUpdated { tvr } => {
                let changed = self.missions.tvr != *tvr;
                if changed {
                    self.missions.tvr = *tvr;
                }
                changed
            }
            AgentEvent::MissionLogUpdated { entry } => {
                // One entry per Port, last frame wins (vault main_menu.md §30).
                match self.missions.logs.iter_mut().find(|e| e.port == entry.port) {
                    Some(existing) if *existing == *entry => false,
                    Some(existing) => {
                        *existing = *entry;
                        true
                    }
                    None => {
                        self.missions.logs.push(*entry);
                        true
                    }
                }
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

/// One target's knockback out of a 0x028: `level` is the wire's 3-bit
/// Knockback (vendor/server/src/map/enums/action/knockback.h), 1..=7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnockbackHit {
    pub target_id: u32,
    pub level: u8,
}

/// Land one slot view at its TradeIndex position (the positional law of vault
/// main_menu.md §25: list index = TradeIndex 0..=8, slot 0 is gil).
fn apply_trade_slot(
    slots: &mut Vec<Option<kuluu_snapshot::TradeOffer>>,
    trade_index: u8,
    offer: Option<kuluu_snapshot::TradeOffer>,
) -> bool {
    let idx = trade_index as usize;
    if idx >= ffxi_proto::decode::TRADE_SLOT_COUNT {
        return false;
    }
    while slots.len() <= idx {
        slots.push(None);
    }
    if slots[idx] == offer {
        return false;
    }
    slots[idx] = offer;
    true
}

/// Any nonzero `resultSet` is result set 1
/// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
pub fn unity_result_set_index(result_set: u8) -> usize {
    usize::from(result_set != 0)
}

/// One s2c 0x08C `merit_t` as sent (vendor/server/src/map/packets/s2c/0x08c_merit.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeritWire {
    pub index: u16,
    pub next: u8,
    pub count: u8,
}

/// One s2c 0x08D `jobpoint_t` as sent (vendor/server/src/map/packets/s2c/0x08d_job_points.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPointWire {
    pub index: u8,
    pub job_no: u16,
    pub next: u16,
    pub level: u8,
}

/// The s2c 0x08C wire id bit that turns an entry into a delete; the wire id halved is the merit
/// index (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
pub const MERIT_WIRE_DELETE_BIT: u16 = 1;
const MERIT_WIRE_INDEX_SHIFT: u32 = 1;

/// The merit index of an s2c 0x08C wire id.
pub fn merit_index_of_wire_id(wire_id: u16) -> u16 {
    wire_id >> MERIT_WIRE_INDEX_SHIFT
}

/// Stores s2c 0x08C entries by merit index: an entry whose wire id has bit 0 set deletes its
/// merit, any other inserts or replaces it.
pub fn fold_merit_entries(
    store: &mut std::collections::BTreeMap<u16, kuluu_snapshot::MeritEntry>,
    entries: &[MeritWire],
) -> bool {
    let mut changed = false;
    for entry in entries {
        let index = merit_index_of_wire_id(entry.index);
        if entry.index & MERIT_WIRE_DELETE_BIT != 0 {
            changed |= store.remove(&index).is_some();
            continue;
        }
        let stored = kuluu_snapshot::MeritEntry {
            next: entry.next,
            count: entry.count,
        };
        changed |= store.insert(index, stored) != Some(stored);
    }
    changed
}

/// Stores s2c 0x08D entries by type id (`job_no * 32 + index`), skipping an entry whose first
/// u16 is 0 or whose job is past the JobPoints record's jobs.
pub fn fold_job_point_entries(
    store: &mut std::collections::BTreeMap<u16, kuluu_snapshot::JobPointType>,
    entries: &[JobPointWire],
) -> bool {
    let mut changed = false;
    for entry in entries {
        let type_id = entry.job_no * ffxi_proto::decode::JobPointWireEntry::TYPES_PER_JOB
            + u16::from(entry.index);
        if type_id == 0 || usize::from(entry.job_no) >= kuluu_snapshot::JOB_POINT_JOBS {
            continue;
        }
        let stored = kuluu_snapshot::JobPointType {
            level: entry.level,
            next: entry.next,
        };
        changed |= store.insert(type_id, stored) != Some(stored);
    }
    changed
}

const _: () = assert!(
    kuluu_snapshot::JOB_POINT_JOBS == ffxi_proto::decode::MiscdataJobPoints::JOBS,
    "the snapshot's JobPoints jobs follow the decoder's"
);

/// Stores one s2c 0x063 Unity chunk into its result set: chunk 1 carries the members, chunk 2
/// the points, and the last chunk completes the set
/// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
pub fn fold_unity_ranking_chunk(
    unity: &mut kuluu_snapshot::UnityState,
    result_set: u8,
    data_type: u8,
    concord_values: &[u32; kuluu_snapshot::UNITY_CONCORDS],
) -> bool {
    use ffxi_proto::decode::MiscdataUnity;
    let Some(set) = unity.rankings.get_mut(unity_result_set_index(result_set)) else {
        return false;
    };
    let before = *set;
    match data_type {
        MiscdataUnity::DATA_TYPE_MEMBERS => set.members = *concord_values,
        MiscdataUnity::DATA_TYPE_POINTS => set.points = *concord_values,
        MiscdataUnity::LAST_CHUNK => set.complete = true,
        _ => {}
    }
    *set != before
}

const _: () = assert!(
    kuluu_snapshot::UNITY_CONCORDS == ffxi_proto::decode::UNITY_CONCORD_COUNT,
    "the snapshot's Concord count follows the decoder's"
);
const _: () = assert!(
    kuluu_snapshot::ALTER_EGO_UPGRADE_SLOTS == ffxi_proto::decode::AlterEgoPoints::UPGRADE_SLOTS,
    "the snapshot's alter ego upgrade slots follow the decoder's"
);

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
    VoyageSynced {
        voyage: kuluu_snapshot::Voyage,
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

    /// 0x068 PetSync is sent to the pet's owner only, so every one we receive
    /// describes our own pet: `Some` is the up variant carrying the pet's
    /// targid, `None` the despawn shape. The server redirects PET-flagged
    /// actions at this pet (vendor/server/src/map/ai/helpers/targetfind.cpp
    /// CTargetFind::getValidTarget TARGET_PET).
    OwnPetSynced {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        targid: Option<u16>,
    },

    NameExtractionMiss {
        miss: NameExtractionMiss,
    },

    /// Partial field patch: resolved by wire id first (the common case — the
    /// patcher knows the wire id), falling back to an `act_index` scan when
    /// only one was given.
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
        /// Self allegiance out of 0x037 `Flags2.BallistaFlg`
        /// (vendor/server/src/map/packets/char_status.cpp) — the only channel
        /// for self (the server skips its own 0x0D,
        /// research/XiPackets/world/server/0x000D).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allegiance: Option<u8>,
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

    /// The running event's displayed frame is down again without the session
    /// ending: dismissal parked on a timed hold, or a pending tag awaits its
    /// ack. Retail clears CliEventMessOpenFlag when the player answers, so the
    /// box hides until the next [`AgentEvent::EventDialog`] reopens it —
    /// otherwise the dismissed line (and its advance hint) lingers over camera
    /// moves and holds.
    DialogDismissed,

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

    /// The shop window went away. There is no close-shop packet — c2s 0x082
    /// SHOP_REQ is deprecated and GM-only (research/XiPackets client 0x0082) —
    /// so every close is a client decision: cancel, walking out of the vendor's
    /// range, or a zone change.
    ShopClosed,

    /// Server appraisal answer to a SHOP_SELL_REQ (s2c 0x03D): `price` is per unit.
    /// `count` is the quantity the request asked for — LSB leaves the packet's
    /// `Count` at 0 (vendor/server/src/map/packets/s2c/0x03d_shop_sell.cpp sets
    /// only Price/PropertyItemIndex/Type), so the session substitutes the
    /// quantity it sent. The appraisal reaches `shop.pending_sale` on the
    /// `ShopUpdated` that follows this event, and the shop window puts the
    /// price on the item's row; retail shows the quote there, not in the chat
    /// log, so `apply_event` records no state change for it.
    ShopSellAppraisal {
        price: u32,
        item_index: u8,
        count: u32,
        #[serde(default)]
        item_no: u16,
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
    /// 0x053 cancel packet (vendor/server/src/map/packets/s2c/0x053_systemmes.cpp)
    /// — the client sees it as its own CHAR_PC status flipping off HEALING
    /// and clears the countdown here.
    LogoutCountdownCancelled,
    EventEnded,

    ActionStarted {
        actor_id: u32,
        action_id: u32,
        action_kind: u8,
        target_id: Option<u32>,
        result: Option<ffxi_proto::melee::MeleeResult>,
        animation: Option<u16>,
        /// First result's outcome bits, read for every category; absent when the body carries
        /// no result block.
        outcome: Option<ffxi_proto::melee::ResultOutcome>,
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

    /// GROUP_TBL (s2c 0x0C8, vendor/server/src/map/packets/s2c/0x0c8_group_tbl.cpp)
    /// arrived: the server is sending a fresh party definition. Clear the party
    /// list and seed it with the skeleton entries from the table; the full
    /// stats follow in GROUP_LIST (0x0DD,
    /// vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp) packets.
    PartyTableReset {
        members: Vec<ffxi_proto::decode::GroupTblEntry>,
    },

    /// s2c 0x0DC GROUP_SOLICIT_REQ: an incoming party/alliance invite awaiting
    /// the c2s 0x074 answer (vendor/server/src/map/packets/s2c/
    /// 0x0dc_group_solicit_req.h).
    PartySolicitReceived {
        char_id: u32,
        name: String,
        alliance: bool,
    },

    /// s2c 0x0DE GROUP_SOLICIT_NO: the pending invite is withdrawn/expired.
    PartySolicitWithdrawn,

    /// s2c 0x021 ITEM_TRADE_REQ: `char_id` asked to trade with us; answer rows
    /// open until c2s 0x033 goes out (vendor/server/src/map/packets/s2c/
    /// 0x021_item_trade_req.h).
    TradeIncoming {
        char_id: u32,
    },

    /// Our own c2s TRADE_REQ was accepted by the transport: open the window in
    /// its outgoing state (retail opens on send, vault main_menu.md section 25).
    TradeOutgoingOpened {
        char_id: u32,
    },

    /// s2c 0x022 ITEM_TRADE_RES kind (Start/Cancell/Make/…/End):
    /// vendor/server/src/map/packets/s2c/0x022_item_trade_res.h.
    TradeRes {
        kind: ffxi_proto::decode::TradeResKind,
    },

    /// s2c 0x025 ITEM_TRADE_MYLIST slot view; `offer` is None for the empty-slot
    /// push (sentinel ItemNo with quantity 0, per 0x025_item_trade_mylist.cpp).
    TradeMySlotSet {
        trade_index: u8,
        offer: Option<kuluu_snapshot::TradeOffer>,
    },

    /// s2c 0x023 ITEM_TRADE_LIST slot view of the partner's side.
    TradePartnerSlotSet {
        trade_index: u8,
        offer: Option<kuluu_snapshot::TradeOffer>,
    },

    /// Our Make went out on the wire — mark us confirmed while the commit lands.
    TradeMadeLocally,

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

    /// s2c 0x058 ASSIST: the server retargeted us. `target_id` is the wire
    /// `AssistNo`; `None` is LSB's zeroed `AssistNo`
    /// (vendor/server/src/map/packets/s2c/0x058_assist.cpp GP_SERV_COMMAND_ASSIST::GP_SERV_COMMAND_ASSIST), i.e. the
    /// target went away rather than moved.
    TargetChanged {
        target_id: Option<u32>,
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

    /// s2c 0x00A LOGIN `PlayTime` — cumulative seconds as the server counted them
    /// at this zone-in (vendor/server/src/map/packets/s2c/0x00a_login.cpp
    /// GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN PlayTime). Retail re-reports it every
    /// zone-in and adds its own elapsed clock on top when `/playtime` runs
    /// (FFXiMain.dll retail-2026-09 handler RVA 0x7CAB0).
    PlayTimeSynced {
        seconds: u32,
    },

    /// s2c 0x062 GP_SERV_COMMAND_CLISTATUS2 — the whole skill block (combat/magic/
    /// craft rows of the Status window, and the only source that can enable those
    /// three rows: FFXiMain.dll retail-2026-09 `.text 0x148B50` opens them disabled).
    CharSkillsUpdated {
        skills: kuluu_snapshot::CharSkills,
    },

    /// s2c 0x063 GP_SERV_COMMAND_MISCDATA subtype **Merits** (bead kuluu-o9qt7.43): gates and
    /// fills the Status window's Merit Points row — retail stores its two dwords at .data
    /// FFXiMain.dll retail-2026-09 0x485826/0x48582A and raises the received flag (.text
    /// dispatcher `0x99270`, case arm `.text 0x99290`; vault main_menu.md §59 addendum 36).
    MeritsUpdated {
        merits: kuluu_snapshot::MisdataMerits,
    },

    /// s2c 0x08E alter ego points: balance, upgrade levels and costs
    /// (vendor/server/src/map/packets/s2c/0x08e_alter_ego_points.h).
    AlterEgoPointsUpdated {
        points: kuluu_snapshot::AlterEgoPoints,
    },

    /// One s2c 0x063 MISCDATA Unity chunk, decoded by LSB's layout
    /// (vendor/server/src/map/packets/s2c/0x063_miscdata_unity.h): `concord_values` holds
    /// `members[11]` / `points[11]` for chunks 1 / 2 and zeros otherwise.
    UnityRankingChunk {
        result_set: u8,
        data_type: u8,
        concord_values: [u32; kuluu_snapshot::UNITY_CONCORDS],
    },

    /// A c2s 0x116 went out for `result_set`, clearing that set's complete mark
    /// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    UnityRankingRequested {
        result_set: u8,
    },

    /// s2c 0x110 GP_SERV_COMMAND_UNITY (vendor/server/src/map/packets/s2c/0x110_unity.h); it arms
    /// the next c2s 0x117.
    UnityQuestUpdated {
        sparks: u32,
        deeds: u16,
        plaudits: u16,
    },

    /// A c2s 0x117 went out, clearing the s2c 0x110 mark.
    UnityQuestRequested,

    /// s2c 0x00A `LoginState` / `IsMonstrosity`, emitted after the zone-in's `ZoneChanged`
    /// (vendor/server/src/map/packets/s2c/0x00a_login.h).
    LoginStateSynced {
        login_state: u32,
        is_monstrosity: bool,
    },

    /// s2c 0x08C GP_SERV_COMMAND_MERIT: its counted entries as sent
    /// (vendor/server/src/map/packets/s2c/0x08c_merit.h).
    MeritsListed {
        entries: Vec<MeritWire>,
    },

    /// s2c 0x08D GP_SERV_COMMAND_JOB_POINTS: all 64 entries as sent
    /// (vendor/server/src/map/packets/s2c/0x08d_job_points.h).
    JobPointsListed {
        entries: Vec<JobPointWire>,
    },

    /// One s2c 0x063 MISCDATA JobPoints record
    /// (vendor/server/src/map/packets/s2c/0x063_miscdata_job_points.h).
    JobPointsRecordUpdated {
        record: kuluu_snapshot::JobPointsRecord,
    },

    /// One s2c 0x063 MISCDATA Homepoints record — the fast-travel access masks (bead
    /// kuluu-o9qt7.45; retail dispatcher `.text FFXiMain.dll retail-2026-09 0x99270`, arm
    /// `.text 0x99330`; vault main_menu.md §59 addendum 37).
    HomepointsUpdated {
        masks: kuluu_snapshot::HomepointMasks,
    },

    /// s2c 0x113/0x118 currency reports (bead kuluu-o9qt7.35), raw body as received — the
    /// mode selects which report family (`kuluu_render::hud::currency_law::CurrencyMode`) and
    /// each arrival replaces its block wholesale, mirroring retail's one-shot reply handler
    /// (FFXiMain.dll retail-2026-09 `.text 0x99640`/`.text 0x99660`).
    CurrenciesLoaded {
        mode: u8,
        body: Vec<u8>,
    },

    /// s2c 0x0F9 `GP_SERV_COMMAND_RES`
    /// (vendor/server/src/map/packets/s2c/0x0f9_res.cpp): `None` restores the
    /// default home-point-only menu; `Some` offers Raise/Reraise or Tractor.
    DeathMenuUpdated {
        offer: Option<ffxi_proto::decode::DeathMenuOffer>,
    },

    MusicVolumeChanged {
        slot: u8,
        volume: u8,
    },

    /// Event script 0xC8 MAP_TUTORIAL (research/XiEvents/OpCodes/0x00C8.md):
    /// open the Map screen on zone `map_id`.
    MapOpen {
        map_id: u16,
        tutorial: bool,
    },

    /// Event script 0x8B MAP_MARKER (research/XiEvents/OpCodes/0x008B.md):
    /// place a named marker at milli-unit coordinates on zone `map_id`'s map.
    MapMarkerPlaced {
        map_id: u16,
        x_milli: i32,
        y_milli: i32,
        label: String,
    },

    /// Event script 0x8A CLOSE_MAP (research/XiEvents/OpCodes/0x008A.md):
    /// close the Map screen.
    MapClosed,

    /// Event scripts 0x69 SETSOUNDVOLUME: drive the masked retail sound channels
    /// to exactly full (`enabled`) or exactly silence at once — no ramp.
    /// kuluu folds retail's four mixer arms plus its global multiplier slot into
    /// two buses (`.agents/skills/retail-observe/references/
    /// 2026-10-28-event-opcode-no-skip-pass.md`).
    EventSoundChannelsSet {
        mask: u8,
        enabled: bool,
    },

    /// Event script 0x6A CHANGESOUNDVOLUME: the masked channels ease to
    /// `volume_thousandths`/1000 — retail's scale constant is exactly 1/1000.
    EventSoundChannelsChange {
        mask: u8,
        volume_thousandths: i32,
    },

    /// Event script 0x8D OPEN_MAP_PROPS: the prop-label layer state of a zone's
    /// map window, both raw work operands (retail passes them to its shared
    /// window helper as select/value). kuluu has no prop-label layer yet, so the
    /// state is carried and stored only (record delta).
    EventMapPropsState {
        select: i32,
        value: i32,
    },

    /// Event scripts 0x34/0x35 MAPLOAD/MAPLOAD_KEEP, third pass: retail tears
    /// the map-window data down and re-opens it for the id its first work
    /// operand names (record delta; kuluu's only observable branch).
    MapReloadRequested {
        map_id: u16,
    },

    /// Event scripts 0x92/0x94 flag-word bits and 0x81 visual-half byte: one
    /// per-actor render-flag write, `bit` the index from the image offsets
    /// (`.agents/skills/retail-observe/references/2026-10-28-event-opcode-no-skip-pass.md`).
    /// Published only — no source names these bits' visual role yet, so nothing
    /// applies them.
    ActorRenderFlagChanged {
        actor: CutsceneActor,
        bit: u32,
        value: bool,
    },

    LevelUp {
        player_id: u32,
    },

    SkillLevelUp {
        skill_id: u16,
        level: u32,
    },

    /// s2c 0x029 with a MsgBasic that
    /// vendor/server/src/map/ai/controllers/player_controller.cpp
    /// CPlayerController::Engage (or entities/char_entity.cpp
    /// CCharEntity::applyTargetRestrictions under it) answers a 0x01A Attack
    /// with instead of the 0x058 accept. Emitted only for our own casts.
    EngageRefused {
        message_num: u16,
    },

    /// Every target result of one s2c 0x028 that carried a knockback level on
    /// a hit (vendor/server/src/map/packets/s2c/0x028_battle2.cpp
    /// GP_SERV_COMMAND_BATTLE2::pack, the 3 knockback bits per result). The
    /// server moves nobody for it; the client shoves the victim when the
    /// skill routine's knockback stage fires.
    Knockbacks {
        actor_id: u32,
        hits: Vec<KnockbackHit>,
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
    /// vendor/server/data/enums/animation.yaml). The server owns this — it
    /// starts and ends resting on its own (damage, status effects) — so the
    /// renderer reconciles its optimistic local stance against it.
    SelfServerStatus {
        status: u8,
        /// 0x037's `mount_id` — which mount, not whether one is being ridden.
        /// It rides this event because both fall out of the same packet and the
        /// pair is only meaningful read together.
        mount_id: u8,
    },

    /// A cutscene's 0x7E mount cue armed the local player's mount on the
    /// client. Unlike [`AgentEvent::SelfServerStatus`], this write is not a
    /// server byte: the cue runs in the event VM and the server only learns of
    /// the mount when its own script adds the MOUNTED effect at event finish
    /// (vendor/server/scripts/effects/mounted.lua). Until that re-assertion
    /// lands as a mounted 0x037 byte, non-mounted 0x037 bytes are stale and
    /// must not clear the cue's write — see `SessionState::cs_mount_armed`.
    CsMountArmed {
        status: u8,
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

    /// Player-search answer: every CSearchListPacket of one query merged in
    /// arrival order; `total` is the server-declared overall count (the last
    /// frame's [0x0E], vault main_menu.md §26).
    PlayerSearchCompleted {
        total: u16,
        players: Vec<kuluu_snapshot::SearchPlayer>,
    },

    /// s2c 0x0E0 GP_SERV_COMMAND_GROUP_COMLINK: the pearl equipped in `linkshell_slot` (1 or 2) now
    /// lives at `category`/`item_index`; zeros mean the slot is empty — LSB leaves both fields zero
    /// when `equipLocation(SLOT_LINK1/SLOT_LINK2)` has no entry. Vault main_menu.md §27.
    LinkShellSlotChanged {
        linkshell_slot: u8,
        category: u8,
        item_index: u8,
    },

    /// The client's Mog House arrangement mode changed — emitted by the session actor after its own
    /// gate chain accepted an [`AgentCommand::MyroomMode`], and when a zone-in drops a mode that was
    /// in force (retail keeps the mode in the myroom manager alone; no wire carries it —
    /// FFXiMain.dll retail-2026-09 `.text 0x1D34FA`). Values are the `MODE_*` constants of
    /// [`crate::myroom_layout`](crate::myroom_layout).
    MyroomModeChanged {
        previous: u8,
        next: u8,
    },

    /// s2c 0x06F GP_SERV_COMMAND_COMBINE_ANS: the answer to our last synthesis
    /// ask replaces whatever we held (vault main_menu.md §28).
    SynthCompleted {
        outcome: kuluu_snapshot::SynthOutcome,
    },

    /// s2c 0x056 port 0xFFFF main frame replaces the mission log whole (vault
    /// main_menu.md §30).
    MissionInfoUpdated {
        info: kuluu_snapshot::MissionInfo,
    },

    /// One s2c 0x044 EXTENDED_JOB PUP frame replaces the automaton record whole
    /// (vendor/server/src/map/packets/s2c/0x044_extended_job_pup.h; vault
    /// main_menu.md §39).
    AutomatonUpdated {
        automaton: kuluu_snapshot::AutomatonInfo,
    },

    /// The s2c 0x0B4 CONFIG answer restates the four SAVE_CONF bitfield bytes
    /// whole (vendor/server/src/common/mmo.h; vault main_menu.md §42).
    ConfigUpdated {
        confdata: [u8; 4],
    },

    /// One s2c 0x044 EXTENDED_JOB MON frame replaces the monstrosity record
    /// whole (vault main_menu.md §39).
    MonstrosityUpdated {
        monstrosity: kuluu_snapshot::MonstrosityInfo,
    },

    /// s2c 0x056 port 0xFFFE `Expansion_TVR` (vault main_menu.md section 30).
    MissionTvrUpdated {
        tvr: u32,
    },

    /// One OTHER-port frame replaces that port's log whole (vault main_menu.md
    /// section 30); unseen ports append in arrival order.
    MissionLogUpdated {
        entry: kuluu_snapshot::MissionLogEntry,
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

    /// Wholesale replacement of the client-local macro list (loaded at world
    /// entry from `macro_store`, or after a commit) — no wire origin, see
    /// vendor/server packet index [V-negative] and vault main_menu.md §41.
    MacrosLoaded {
        slots: Vec<kuluu_snapshot::MacroSlot>,
    },

    /// Wholesale replacement of this character's emote display order (bead
    /// kuluu-o9qt7.34) — world entry load or commit echo; never a wire origin.
    EmoteOrderLoaded {
        ids: Vec<u8>,
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
    /// Answer to the pending party invite (`prtyjoin` YES/NO in retail,
    /// FFXiMain.dll retail-2026-09 RVA 0x2016E0 opens it; vault main_menu.md
    /// section 24). Sends c2s GROUP_SOLICIT_RES (vendor/server/src/map/packets/
    /// c2s/0x074_group_solicit_res.h: Res { Decline = 0, Accept = 1 }).
    PartyInviteRespond {
        accept: bool,
    },

    /// Ask `char_id` to trade — c2s 0x032 TRADE_REQ (UniqueNo + the target's
    /// wire ActIndex, both read off the snapshot entity by the caller). The
    /// server pushes the target s2c 0x021; nothing changes for us until they
    /// answer.
    TradeRequest {
        char_id: u32,
        act_index: u16,
    },

    /// Answer an incoming trade request (c2s 0x033 TRADE_RES Kind Start/Cancell).
    TradeRespond {
        accept: bool,
    },

    /// Stage one bag slot into our offer (c2s 0x034 TRADE_LIST): qty ItemNum,
    /// `item_no` must match the bag slot's contents (player_trade.cpp setSlot).
    TradeStage {
        trade_index: u8,
        item_no: u16,
        qty: u32,
        bag_slot: u8,
    },

    /// Offer gil — TRADE_LIST on slot 0 with the GIL sentinel item id (LSB's
    /// `GilSlot = 0`, validate() rejects it elsewhere).
    TradeSetGil {
        qty: u32,
    },

    /// Clear one of our offer slots: TRADE_LIST with ItemNum 0 (setSlot takes
    /// that as the release path).
    TradeClearSlot {
        trade_index: u8,
    },

    /// Confirm our side (c2s 0x033 Kind Make); the commit resolves when both
    /// sides made and the server sends RES End to each.
    TradeConfirm,

    /// Walk away (c2s 0x033 Kind Cancell).
    TradeCancel,

    /// Run a `searchma` query against the search server: one TCP_SEARCH (or
    /// TCP_SEARCH_ALL) request whose TLV blob is built from these filters
    /// (vendor/server/src/search/search_handler.cpp _HandleSearchRequest; vault
    /// main_menu.md §26). The merged result frames come back as
    /// [`AgentEvent::PlayerSearchCompleted`].
    PlayerSearch {
        filters: Vec<ffxi_proto::search::SearchFilter>,
        #[serde(default)]
        all_pages: bool,
    },

    /// Ask one synthesis (c2s 0x096 COMBINE_ASK): the crystal's item id plus its bag slot, and
    /// the ingredient (item id, bag slot) pairs already in the recipe row's own order — that
    /// positional order is what the server keys on (`resolveRecipe`, vault main_menu.md §28).
    Synthesize {
        crystal_id: u16,
        crystal_slot: u8,
        ingredients: Vec<ffxi_proto::synth::SynthIngredient>,
    },

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

    /// One select on myroom's door row (select id 6 of handler FFXiMain.dll retail-2026-09
    /// `.text 0x145460`, arm @`.text 0x14556C`). The arm branches on the client-side door byte at
    /// game-state `+0x40C40` (predicate `.text 0xF96E0`): closed door -> MYROOM_IS{Kind=Open,
    /// Param2=Unk1(0)}, open door -> MYROOM_IS{Kind=Close, Param2=Unk2(1)} - the Kind and PARAM2
    /// values in vendor/server/src/map/packets/c2s/0x0cb_myroom_is.h. kuluu answers that same branch
    /// from [`MyRoomInfo::door_open`] and flips it optimistically: LSB implements neither Kind
    /// (`process()` is literally "Not implemented" twice in
    /// vendor/server/src/map/packets/c2s/0x0cb_myroom_is.cpp) and ships no s2c 0x067, so no wire
    /// could ever disagree with the client's own byte. Which rows the update slot greys while the
    /// door is open (Layout / Remodel) lives in kuluu-render's `menu_row_disabled`.
    MogHouseDoorSelect,

    /// One confirmed `roomlist` style row: c2s 0x0CB MYROOM_IS Kind=Remodel with
    /// Param2 = `style` (615 Sandorian / 616 Bastokan / 617 Windurstian / 618 Mog
    /// Patio — GP_CLI_COMMAND_MYROOM_IS_PARAM2). Retail's sender (.text FFXiMain.dll
    /// retail-2026-09 0x1D6AD0) pairs it with Param1 = byte[roomlist obj+0x50]; LSB never
    /// reads that field — validate() checks only Kind/Param2 and process() acts on Param2
    /// alone (vendor/server/src/map/packets/c2s/0x0cb_myroom_is.cpp) — so kuluu sends
    /// Param1=0. A confirmed style change answers MsgStd::SuccessfulRemodel, and when the
    /// model actually changes while on MH2F (mhflag&0x40) LSB forces a same-zone rezone,
    /// which kuluu's generic zone-change path already follows (vault main_menu.md §59
    /// addendum 20/21; bead kuluu-o9qt7.13.5).
    MogHouseRemodel {
        style: u16,
    },

    /// One furniture placement/move inside a Mog House: c2s 0x0FA MYROOM_LAYOUT, built by sender
    /// helper FFXiMain.dll retail-2026-09 `.text 0xF0CC0` (see [`crate::session::codec::
    /// MyroomLayoutRequest`] for what each byte means and for the ranges LSB validates). Nothing is
    /// updated optimistically: a placement that survives validation comes straight back as s2c 0x020
    /// ITEM_ATTR, which is the only thing kuluu treats as furniture state
    /// ([`crate::myroom_furniture`]; the client's own handler for the 0x0FA echo at `.text 0xF0C70` is
    /// `mov al, 1; ret`).
    MyroomLayout {
        item_no: u16,
        slot: u8,
        container: u8,
        floor: u8,
        col: i8,
        level: i8,
        row: i8,
        rotation: i8,
    },

    /// The closing beat of layout mode: the same 0x0FA packet with every field zero
    /// (FFXiMain.dll retail-2026-09 `.text 0xF0C80`), sent when leaving layout mode for normal mode so
    /// LSB re-runs `UpdateMoghancement` (vendor/server/src/map/packets/c2s/
    /// 0x0fa_myroom_layout.cpp, `if (this->MyroomItemNo == 0)`).
    MyroomLayoutDone,

    /// Take one placed piece back onto the Mog House shelf: c2s 0x0FB MYROOM_BANKIN, sender helper
    /// FFXiMain.dll retail-2026-09 `.text 0xF0D40`. What else goes with it is server law, not a guess:
    /// LSB clears Installed and the whole pose, empties a flowerpot it was holding
    /// (`CItemFlowerpot::cleanPot`) and unlocks a mannequin's outfit — each of those arrivals is its own
    /// ITEM_ATTR/ITEM_LIST record, so kuluu assumes nothing locally (the client likewise only queues the
    /// request; when it removes a surface it marks whatever sat on it dirty too,
    /// `.text 0x1CE0F0`). vendor/server/src/map/packets/c2s/0x0fb_myroom_bankin.cpp.
    MyroomBankin {
        item_no: u16,
        slot: u8,
        container: u8,
    },

    /// Sow a seed / feed a plant: c2s 0x0FC MYROOM_PLANT_ADD, sender helper FFXiMain.dll retail-2026-09
    /// `.text 0xF0DA0` (vendor/server/src/map/packets/c2s/0x0fc_myroom_plant_add.h).
    MyroomPlantAdd {
        plant_item_no: u16,
        add_item_no: u16,
        plant_slot: u8,
        add_slot: u8,
        plant_container: u8,
        add_container: u8,
    },

    /// Examine a growing plant: c2s 0x0FD MYROOM_PLANT_CHECK, sender helper FFXiMain.dll retail-2026-09
    /// `.text 0xF0E10`.
    MyroomPlantCheck {
        plant_item_no: u16,
        plant_slot: u8,
        plant_container: u8,
    },

    /// Harvest a plant. `cancel = true` is the header's `CancellFlg = 1`, which retail sends when the
    /// player backs out of the harvest prompt (call site FFXiMain.dll retail-2026-09 `.text 0x206951`);
    /// a real harvest passes 0 (`.text 0x2069B1`).
    MyroomPlantCrop {
        plant_item_no: u16,
        plant_slot: u8,
        plant_container: u8,
        cancel: bool,
    },

    /// Stop drying/harvesting a plant: c2s 0x0FF MYROOM_PLANT_STOP, sender helper FFXiMain.dll
    /// retail-2026-09 `.text 0xF0ED0`.
    MyroomPlantStop {
        plant_item_no: u16,
        plant_slot: u8,
        plant_container: u8,
    },

    /// Enter or leave one of the Mog House arrangement modes: the mode switch FFXiMain.dll retail-2026-09
    /// `.text 0x1D3380(mode, slot, container)` with no particular piece asked for (retail spells that as a
    /// slot of -1 at `.text 0x7E78D`/`.text 0x7E999`). 0 returns to normal play, 1 is `/layout`, 2 is
    /// `/garden`; the refusal law, the windows it closes and the wire beat it may owe all come from
    /// [`crate::myroom_layout`] [V].
    MyroomMode {
        mode: u8,
    },

    /// Client-local open of the same menu s2c 0x02E OPENMOGMENU triggers
    /// (vendor/server/src/map/packets/s2c/0x02e_openmogmenu.h).
    OpenMogMenu,

    /// Commit the character's whole macro set: persisted to the client-local
    /// store file and folded into state together, so runtime and user file can
    /// never disagree (bead kuluu-o9qt7.19.2; retail keeps macros in its own
    /// per-user files, vault main_menu.md §41/§53).
    CommitMacros {
        slots: Vec<kuluu_snapshot::MacroSlot>,
    },

    /// Rewrite the character's emote display order as a whole: persisted to the
    /// client-local store and folded into state together, exactly like
    /// `CommitMacros` (bead kuluu-o9qt7.34; retail's user-file law from
    /// FFXiMain.dll retail-2026-09 `.text 0x2259C6`, no wire).
    CommitEmoteOrder {
        ids: Vec<u8>,
    },

    /// Send the header-only currency report request for one Currencies window — c2s 0x10F or
    /// 0x115 by `mode` wire discriminant (0/1, `CurrencyMode::as_wire`; answers arrive as s2c
    /// 0x113/0x118). Retail issues one per window open and caches nothing (FFXiMain.dll
    /// retail-2026-09 `.text 0xFD520`/`.text 0xFD550`; vendor/server/src/map/packets/c2s/
    /// 0x10f_currencies_1.cpp answers straight back — bead kuluu-o9qt7.35).
    RequestCurrencies {
        mode: u8,
    },

    /// c2s 0x116 GP_CLI_COMMAND_UNITY_MENU `Kind = result_set`
    /// (vendor/server/src/map/packets/c2s/0x116_unity_menu.h); LSB answers with s2c 0x110 and both
    /// ranking sets. The `ut_menu` window sends it once per set whose table completed since the
    /// last request (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    RequestUnityRanking {
        result_set: u8,
    },

    /// c2s 0x117 GP_CLI_COMMAND_UNITY_QUEST `Kind = 0`
    /// (vendor/server/src/map/packets/c2s/0x117_unity_quest.h), sent when a ranking list opens
    /// after an s2c 0x110.
    RequestUnityQuest,

    /// c2s 0x118 GP_CLI_COMMAND_UNITY_TOGGLE `Mode`
    /// (vendor/server/src/map/packets/c2s/0x118_unity_toggle.h): 1 joins Unity chat, 0 leaves it.
    SetUnityChat {
        mode: u8,
    },

    /// The header-only c2s 0x0C2 alter ego points request, sent when the alter ego point windows
    /// open before any s2c 0x08E (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    RequestAlterEgoPoints,

    /// c2s 0x0C1 GP_CLI_COMMAND_ALTER_EGO_POINTS `CategoryIndex = upgrade_id`
    /// (vendor/server/src/map/packets/c2s/0x0c1_alter_ego_points.h).
    UpgradeAlterEgo {
        upgrade_id: u16,
    },

    /// c2s 0x0BE GP_CLI_COMMAND_MERITS ChangeMode (vendor/server/src/map/packets/c2s/0x0be_merits.h):
    /// `limit_points` false asks for EXP mode, true for Limit Points mode.
    SetMeritMode {
        limit_points: bool,
    },

    /// c2s 0x0BE GP_CLI_COMMAND_MERITS EditMode: raise (`raise`) or lower merit `index` (the wire
    /// id halved).
    EditMerit {
        index: u16,
        raise: bool,
    },

    /// c2s 0x0BF GP_CLI_COMMAND_JOB_POINTS_SPEND on job point type `type_id`
    /// (vendor/server/src/map/packets/c2s/0x0bf_job_points_spend.h).
    SpendJobPoint {
        type_id: u16,
    },

    /// The header-only c2s 0x0C0 GP_CLI_COMMAND_JOB_POINTS_REQ
    /// (vendor/server/src/map/packets/c2s/0x0c0_job_points_req.h).
    RequestJobPoints,

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

    /// The renderer finished (or could not start) the 0x2C SCHEDULOR routine
    /// (research/XiEvents/OpCodes/0x002C.md) this `(actor, key)` named:
    /// releases the event VM's pending hold on it.
    /// Carries the wire actor the cue named, so the session matches the same
    /// value it resolved the cue with.
    CutsceneMotionDone {
        actor: CutsceneActor,
        key: ffxi_event::FourCc,
    },

    EndEvent,

    /// One step back out of a client-local menu tree (the Mog House Moogle and
    /// exit-door menus): pop a level, or end the interaction at the root.
    /// Retail backs out one level per Esc and closes only from the root
    /// (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md).
    /// A server-driven event has no levels, so this ends it exactly as
    /// [`AgentCommand::EndEvent`] does.
    EndEventBack,

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

    /// Wear one's whole equipment set: c2s 0x051 (`equipset::equipset_set_body` packs the populated entries;
    /// the server runs every equip call, so appearance updates arrive back as normal equipment updates - the
    /// client applies nothing locally, `.agents/skills/retail-observe/references/2026-10-25-equip-set-apply.md`).
    /// `entries` is the set record's contents half in order.
    EquipSetWear {
        entries: [ffxi_proto::equipset::EquipSetEntry; ffxi_proto::equipset::EQUIPSET_ENTRIES],
    },

    /// Lock one's style to an equipment set: c2s 0x053 Mode = Set. Entries past the feet position are dropped
    /// by the encoder, whichever arm hands them over.
    StyleLockSet {
        entries: [ffxi_proto::equipset::EquipSetEntry; ffxi_proto::equipset::EQUIPSET_ENTRIES],
    },

    /// Report one edit to a set's contents: c2s 0x052, the edited `position` (LSB bounds it 0..15), the entry
    /// just placed or removed, and the whole list as now stored. Nothing is worn by this packet.
    EquipSetCheck {
        position: u8,
        change: ffxi_proto::equipset::EquipSetEntry,
        contents: [ffxi_proto::equipset::EquipSetEntry; ffxi_proto::equipset::EQUIPSET_ENTRIES],
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

    /// c2s 0x028 ITEM_DUMP: throw `quantity` of the item at `container`/`index`
    /// away (retail's Items > Drop). Gil, locked slots and filled storage slips
    /// are refused server-side
    /// (vendor/server/src/map/packets/c2s/0x028_item_dump.cpp process).
    DropItem {
        container: u8,
        index: u8,
        quantity: u32,
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

    /// Walk back from an appraised sale without completing it. Sends nothing:
    /// the server parks the item in its shop container on SHOP_SELL_REQ and
    /// only moves it on SHOP_SELL_SET
    /// (vendor/server/src/map/packets/c2s/0x085_shop_sell_set.cpp process), so
    /// declining is just the client dropping the quote.
    ShopSellCancel,

    /// Drop the open shop window. Sends nothing — the shop has no close packet
    /// (research/XiPackets client 0x0082 is deprecated and GM-only), so this is
    /// purely the client forgetting the stock it was shown.
    CloseShop,

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

    /// Raise our own stall's price menu (c2s 0x10B with AllListClearFlg 0): LSB
    /// sets `isSettingBazaarPrices` and drops our customer list, so buyers leave the
    /// stall we are about to reprice
    /// (vendor/server/src/map/packets/c2s/0x10b_bazaar_close.cpp).
    BazaarPricesEnter,

    /// Price one LOC_INVENTORY slot of our own stall (c2s 0x10A). LSB clamps the
    /// range to 0..=BAZAAR_PRICE_MAX, rejects currency and busy items outright, and
    /// answers ITEM_ATTR + ITEM_SAME for that slot
    /// (vendor/server/src/map/packets/c2s/0x10a_bazaar_itemset.cpp).
    BazaarPriceSet {
        slot: u8,
        price: u32,
    },

    /// Leave the price menu (c2s 0x109): LSB clears `isSettingBazaarPrices` and sets
    /// UPDATE_HP, which is what republishes us as a stall with bazaar flag set
    /// (vendor/server/src/map/packets/c2s/0x109_bazaar_open.cpp).
    BazaarPricesDone,

    /// Publish the message buyers read when they check our stall (c2s 0x0DE). LSB stores it
    /// verbatim in `char_stats.bazaar_message`, answers nothing, and only ever shows it back to
    /// us as s2c 0x0CA INSPECT_MESSAGE when somebody checks us
    /// (vendor/server/src/map/packets/c2s/0x0de_inspect_message.cpp).
    BazaarComment {
        text: String,
    },

    /// Flip one server-visible option flag (c2s 0x0DC GP_CLI_COMMAND_CONFIG;
    /// vendor/server/src/map/packets/c2s/0x0dc_config.cpp). No optimistic fold:
    /// LSB always answers with a fresh s2c 0x0B4 CONFIG (vault main_menu.md
    /// §42), and that echo reconciles the snapshot.
    SetConfigFlag {
        flag: ffxi_proto::config::ConfigBit,
        on: bool,
    },

    /// Announce one flip of the `conf12wi` store-0xBF pair (c2s **0x088**, body
    /// law FFXiMain.dll retail-2026-09 RVA 0xE5520; vault main_menu.md §59
    /// addendum 16). The mode byte is already the law-derived transition value —
    /// the helper's Monstrosity bail (.text 0x1A8036) and its >0x2328 ms guard
    /// (.text 0x1A7D05) run in the menu seam before this is sent. vendor/server
    /// carries no handler for c2s 0x088 (packets/c2s/ has no file for the id),
    /// so LSB drops the frame and nothing folds back from it.
    AnnounceConfigBfPair {
        mode: u8,
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

    /// The action ids vendor/server/src/map/packets/c2s/0x01a_action.cpp
    /// GP_CLI_COMMAND_ACTION::validate refuses with BlockedState::InEvent.
    /// Exhaustive on purpose: a new action must state its answer.
    pub fn blocked_in_event(&self) -> bool {
        match self {
            ActionKind::Attack
            | ActionKind::CastMagic { .. }
            | ActionKind::JobAbility { .. }
            | ActionKind::Shoot
            | ActionKind::Weaponskill { .. }
            | ActionKind::MonsterSkill { .. }
            | ActionKind::Fish
            | ActionKind::Mount { .. } => true,
            ActionKind::Talk
            | ActionKind::AttackOff
            | ActionKind::Help
            | ActionKind::HomepointMenu { .. }
            | ActionKind::Assist
            | ActionKind::RaiseMenu { .. }
            | ActionKind::ChangeTarget
            | ActionKind::ChocoboDig
            | ActionKind::Dismount
            | ActionKind::TractorMenu { .. }
            | ActionKind::SendResRdy
            | ActionKind::Quarry
            | ActionKind::Sprint
            | ActionKind::Scout
            | ActionKind::Blockaid { .. } => false,
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

/// `LinkshellNum`/`LinkshellId` values LSB accepts in this family (`GP_CLI_COMMAND_GROUP_COMLINK_*_LINKSHELLID`).
fn linkshell_slot_for(id: u8) -> Option<u8> {
    match id {
        ffxi_proto::linkshell::LINKSHELL_1 => Some(1),
        ffxi_proto::linkshell::LINKSHELL_2 => Some(2),
        _ => None,
    }
}
