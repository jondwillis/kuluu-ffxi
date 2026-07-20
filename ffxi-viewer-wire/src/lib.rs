#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

// v3: SceneSnapshot emote_jobs/emote_chairs + ViewerEvent::EntityEmoted
// (postcard frames are not self-describing, so any shape change bumps this).
pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Position {
    pub pos: Vec3,

    pub heading: u8,

    pub speed: u8,

    pub speed_base: u8,
}

impl Default for Position {
    fn default() -> Self {
        Self {
            pos: Vec3::default(),
            heading: 0,
            speed: 25,
            speed_base: 25,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
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

// vendor/server/src/map/enums/weather.h:24-46 (None=0..Darkness=19)
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Weather {
    #[default]
    None,
    Sunshine,
    Clouds,
    Fog,
    HotSpell,
    HeatWave,
    Rain,
    Squall,
    DustStorm,
    SandStorm,
    Wind,
    Gales,
    Snow,
    Blizzards,
    Thunder,
    Thunderstorms,
    Auroras,
    StellarGlare,
    Gloom,
    Darkness,
}

impl Weather {
    pub fn from_lsb(n: u16) -> Self {
        use Weather::*;
        // vendor/server/src/map/enums/weather.h:24-46
        const TABLE: [Weather; 20] = [
            None,
            Sunshine,
            Clouds,
            Fog,
            HotSpell,
            HeatWave,
            Rain,
            Squall,
            DustStorm,
            SandStorm,
            Wind,
            Gales,
            Snow,
            Blizzards,
            Thunder,
            Thunderstorms,
            Auroras,
            StellarGlare,
            Gloom,
            Darkness,
        ];
        // weather.h:46 notes a repeating 0x14-0x27 set whose usage is unknown;
        // do not fabricate a real weather for undefined ids.
        TABLE.get(n as usize).copied().unwrap_or(Weather::None)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntityLook {
    Standard {
        modelid: u16,
    },
    Equipped {
        face: u8,
        race: u8,
        head: u16,
        body: u16,
        hands: u16,
        legs: u16,
        feet: u16,
        main: u16,
        sub: u16,
        ranged: u16,
    },
    Door {
        size: u16,
    },
    Transport {
        size: u16,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: u32,
    pub act_index: u16,
    pub kind: EntityKind,
    pub name: Option<String>,
    pub pos: Vec3,
    pub heading: u8,
    pub hp_pct: Option<u8>,
    pub bt_target_id: u32,

    /// Head-look target: the targid (act_index) this entity has selected. Drives
    /// the non-self head turn in the renderer. See `state::Entity::face_target`.
    #[serde(default)]
    pub face_target: u16,

    #[serde(default)]
    pub claim_id: u32,

    #[serde(default)]
    pub speed: u8,

    #[serde(default)]
    pub speed_base: u8,

    #[serde(default)]
    pub look: Option<EntityLook>,

    #[serde(default)]
    pub animation: u8,

    /// `!= 0` marks an effect NPC (brazier/lamp/torch flame). See
    /// `ffxi_proto::decode::NpcState`.
    #[serde(default)]
    pub animationsub: u8,

    #[serde(default)]
    pub status: u8,
}

// LSB STATUS_TYPE. vendor/server/src/map/entities/baseentity.h
mod status_type {
    pub const DISAPPEAR: u8 = 2;
    pub const INVISIBLE: u8 = 3;
    pub const STATUS_4: u8 = 4;
    pub const CUTSCENE_ONLY: u8 = 6;
    pub const STATUS_18: u8 = 18;
    pub const SHUTDOWN: u8 = 20;
}

impl Entity {
    pub fn is_dead(&self) -> bool {
        self.hp_pct == Some(0)
    }

    // Blacklist (not whitelist) so an undecoded byte fails open, staying targetable.
    fn status_selectable(&self) -> bool {
        use status_type::*;
        !matches!(
            self.status,
            DISAPPEAR | INVISIBLE | STATUS_4 | CUTSCENE_ONLY | STATUS_18 | SHUTDOWN
        )
    }

    /// Selectable by click / `<t>`. Dead players stay selectable so a healer can
    /// target them to Raise; dead mobs/NPCs do not.
    pub fn is_targetable(&self) -> bool {
        if matches!(self.kind, EntityKind::Other) || !self.status_selectable() {
            return false;
        }
        !self.is_dead() || matches!(self.kind, EntityKind::Pc)
    }

    /// Eligible for the Tab enemy-cycle: targetable and alive. No corpse cycles,
    /// even an ally's.
    pub fn is_cycle_candidate(&self) -> bool {
        self.is_targetable() && !self.is_dead()
    }
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

    /// Chat kind 8 MESSAGE_EMOTION: canned-emote lines (caster name already
    /// embedded in `text`, `sender` empty) and free-form /em (`sender` set,
    /// `text` is the raw emote body).
    Emote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLine {
    pub channel: ChatChannel,
    pub sender: String,
    pub text: String,
    pub server_ts: u32,

    #[serde(default)]
    pub local_seq: u64,
}

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

    #[serde(default)]
    pub in_mog_house: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub stage: Option<Stage>,
    pub blowfish_status: Option<BlowfishStatus>,
    pub sync_in: Option<u16>,
    pub sync_out: Option<u16>,
    pub last_server_packet_age_ms: Option<u64>,
    pub map_server_addr: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetStats {
    pub send_bps: u32,
    pub recv_bps: u32,
    pub send_health: u8,
    pub recv_health: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReactorGoal {
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
        waypoints_remaining: u32,
    },
    Banking {
        threshold: u8,
        mog_house_zoneline: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectInfo {
    pub downtime_ms: u64,
    pub at_unix_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneSnapshot {
    pub stage: Stage,
    pub char_name: Option<String>,
    pub zone_id: Option<u16>,
    pub self_pos: Position,
    pub entities: Vec<Entity>,
    pub party: Vec<PartyMember>,

    pub chat: Vec<ChatLine>,
    pub diagnostics: Diagnostics,

    #[serde(default)]
    pub net_stats: NetStats,

    pub current_goal: Option<ReactorGoal>,

    pub last_reconnect: Option<ReconnectInfo>,

    pub producer_monotonic_ms: u64,

    #[serde(default)]
    pub self_char_id: Option<u32>,

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
    pub weather: Option<Weather>,

    #[serde(default = "default_equipped")]
    pub equipped: [Option<u16>; 16],

    #[serde(default)]
    pub spells_known: Vec<u16>,

    #[serde(default)]
    pub job_abilities_known: Vec<u16>,

    #[serde(default)]
    pub weaponskills_known: Vec<u16>,

    #[serde(default)]
    pub pet_abilities_known: Vec<u16>,

    /// Owned key-item ids (s2c 0x055 GetItemFlag bitsets), sorted ascending —
    /// global id = table * 512 + bit.
    #[serde(default)]
    pub key_items: Vec<u16>,

    /// Subset of `key_items` already examined (LookItemFlag); an owned id not
    /// in here renders the unseen ("new") indicator.
    #[serde(default)]
    pub key_items_seen: Vec<u16>,

    /// Every known item container (main bag + Mog House/global storage), sorted
    /// by container id. Ids are LSB CONTAINER_ID (`ffxi_proto::map::container`).
    #[serde(default)]
    pub containers: Vec<ContainerView>,

    #[serde(default)]
    pub stats: Option<CharStats>,

    #[serde(default)]
    pub bazaar: Vec<BazaarEntry>,

    #[serde(default)]
    pub play_time_s: u64,

    /// Self fishing state, present while the player is fishing. Drives the self pose and
    /// the mini-game HUD.
    #[serde(default)]
    pub self_fishing: Option<SelfFishing>,

    /// `Some` while the server has the player inside a Mog House (same zone_id as
    /// the surrounding city); the renderer must re-key zone resources on it.
    #[serde(default)]
    pub myroom: Option<MyRoom>,

    /// Whether the Mog House 2nd floor is unlocked (0x055 char sync); gates the
    /// Mog Safe 2 bag — the server drops moves into it without profile.mhflag
    /// bit 0x20 (0x029_item_move.cpp validContainers). `None` = not yet known.
    #[serde(default)]
    pub mh_2f_unlocked: Option<bool>,

    /// Job-emote unlock bitfield from s2c 0x11A (bit = job id - 1, bit 0 =
    /// WAR); `None` until the server answers a 0x119 request. Gates the
    /// emote-list menu's Job row.
    #[serde(default)]
    pub emote_jobs: Option<u32>,

    /// Chair unlock bitfield from s2c 0x11A (/sitchair; unused until chairs
    /// exist client-side).
    #[serde(default)]
    pub emote_chairs: Option<u16>,

    /// Accumulated /check answer (s2c 0x0C9 EQUIP_INSPECT) for the last checked
    /// PC; drives the Check panel grid and job ribbon.
    #[serde(default)]
    pub check: Option<CheckResult>,
}

/// `equipped` is indexed by SAVE_EQUIP_KIND slot id (0 = Main .. 15 = Back);
/// jobs are zero while the target is /anon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub target_id: u32,
    pub equipped: [Option<u16>; 16],
    pub main_job: u8,
    pub sub_job: u8,
    pub main_job_lv: u8,
    pub sub_job_lv: u8,
    pub master_lv: u8,
}

impl SceneSnapshot {
    pub fn container(&self, id: u8) -> Option<&ContainerView> {
        self.containers.iter().find(|c| c.id == id)
    }

    /// Container 0 (the main inventory bag), or empty if not yet received.
    pub fn inventory_main(&self) -> &[InventoryItem] {
        self.container(0).map(|c| c.items.as_slice()).unwrap_or(&[])
    }

    /// Whether the self player is inside their Mog House: the s2c 0x00A myroom
    /// cluster wins, otherwise the self party member's moghouse flag. Mirrors
    /// `SessionState::self_in_mog_house` on the producer side.
    pub fn self_in_mog_house(&self) -> bool {
        if self.myroom.is_some() {
            return true;
        }
        let Some(char_id) = self.self_char_id else {
            return false;
        };
        self.party
            .iter()
            .find(|m| m.id == char_id)
            .map(|m| m.in_mog_house)
            .unwrap_or(false)
    }
}

/// s2c 0x00A myroom cluster; `model` is an interior model id, not a zone id —
/// resolve via `ffxi_dat::zone_dat::effective_zone_dat_file_id`
/// (vendor/server/src/map/packets/s2c/0x00a_login.cpp:32-34).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyRoom {
    pub model: u16,
    pub sub_map: u8,
}

/// On-screen fishing arrow during the active mini-game state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FishingArrow {
    pub left: bool,
    pub golden: bool,
}

/// Self fishing view for the renderer/HUD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfFishing {
    /// Macro-state phase 0..=6 for self pose selection (see `ffxi_actor::fishing_clip`).
    pub phase: u8,
    /// Fish max stamina, present once a fish bites (for the HUD bar denominator).
    pub fish_max: u16,
    /// Current fish stamina, for the HUD bar.
    pub fish_hp: u16,
    /// The arrow the player must react to, if any.
    pub arrow: Option<FishingArrow>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharStats {
    pub item_level: u16,
    pub str_: u16,
    pub dex: u16,
    pub vit: u16,
    pub agi: u16,
    pub int_: u16,
    pub mnd: u16,
    pub chr: u16,
    // Self-stat block from s2c 0x061 (CLISTATUS). `bonus` is the signed gear/buff
    // delta retail renders as "+N"; `resist` is the 8 elemental defenses. New fields
    // default so older postcard frames still deserialize.
    #[serde(default)]
    pub hp_max: u32,
    #[serde(default)]
    pub mp_max: u32,
    #[serde(default)]
    pub attack: u16,
    #[serde(default)]
    pub defense: u16,
    #[serde(default)]
    pub bonus: [i16; 7],
    #[serde(default)]
    pub resist: [i16; 8],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BazaarEntry {
    pub item_no: u16,
    pub quantity: u32,
    pub price: u32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryItem {
    pub container: u8,
    pub index: u8,
    pub item_no: u16,
    pub quantity: u32,
    /// LSB lock flag (equipped / linkshell / bazaar-reserved): the server
    /// rejects moving locked items (0x029_item_move.cpp isValidMovement).
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContainerView {
    pub id: u8,
    pub capacity: u16,
    pub items: Vec<InventoryItem>,
}

fn default_equipped() -> [Option<u16>; 16] {
    [None; 16]
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

    #[serde(default)]
    pub prompt: Option<String>,

    #[serde(default)]
    pub choices: Vec<String>,

    /// Free-text entry frame (e.g. the delivery-box recipient prompt):
    /// the viewer collects a line of text and answers with
    /// `AgentCommand::TextInput` instead of a menu choice.
    #[serde(default)]
    pub text_entry: bool,
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
pub struct SceneDelta {
    pub stage: Option<Stage>,
    pub zone_id: Option<u16>,
    pub self_pos: Option<Position>,
    pub entities_upserted: Vec<Entity>,
    pub entities_removed: Vec<u32>,
    pub party_upserted: Vec<PartyMember>,
    pub chat_appended: Vec<ChatLine>,
    pub diagnostics: Option<Diagnostics>,

    /// `Some` = enter/update the Mog House view; `None` = no change (matching
    /// `zone_id`'s merge convention — a delta cannot clear it, so MH exit must
    /// arrive as a full snapshot, which every producer today sends anyway).
    #[serde(default)]
    pub myroom: Option<MyRoom>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewerEvent {
    ZoneChanged {
        from: Option<u16>,
        to: u16,
    },
    EntityRemoved {
        id: u32,
    },
    Disconnected {
        reason: String,
    },
    LowHp {
        pct: u8,
    },
    EngagedBy {
        entity_id: u32,
    },
    TellReceived {
        from: String,
        text: String,
    },
    Reconnected {
        downtime_ms: u64,
    },

    MusicChanged {
        slot: u8,
        track_id: u16,
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

    ActionStarted {
        actor_id: u32,
        action_id: u32,
        action_kind: u8,
    },

    /// One-shot emote broadcast (s2c 0x05A MOTIONMES): `emote_id` is the wire
    /// MesNum (job emotes arrive as 74..=95), `mode` the EmoteMode byte.
    EntityEmoted {
        actor_id: u32,
        target_id: u32,
        emote_id: u16,
        param: u16,
        mode: u8,
    },

    VanaTimeSynced {
        game_time: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Frame {
    Hello { protocol_version: u32 },
    Snapshot(Box<SceneSnapshot>),
    Delta(Box<SceneDelta>),
    Event(ViewerEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewerCommand {
    Move {
        x: f32,
        y: f32,
        z: f32,
        heading: u8,
    },
    StopMove,
    EndEvent,
    Snapshot,
    Chat {
        kind: u8,
        text: String,
    },
    Tell {
        to: String,
        text: String,
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
    },
    Cancel,

    Cast {
        spell_id: u32,
        target_id: u32,
        target_index: u16,
        pos_x: f32,
        pos_y: f32,
        pos_z: f32,
    },

    Weaponskill {
        skill_id: u32,
        target_id: u32,
        target_index: u16,
    },

    JobAbility {
        ability_id: u32,
        target_id: u32,
        target_index: u16,
    },

    UseItem {
        container: u8,
        slot: u8,
        item_no: u32,
        target_id: u32,
        target_index: u16,
    },

    /// c2s 0x029 ITEM_MOVE: `to_slot: None` lets the server pick a free slot.
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientFrame {
    Hello { protocol_version: u32 },
    Command(ViewerCommand),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> SceneSnapshot {
        SceneSnapshot {
            stage: Stage::InZone,
            char_name: Some("Sylvie".into()),
            zone_id: Some(230),
            self_pos: Position {
                pos: Vec3 {
                    x: -10.5,
                    y: 0.0,
                    z: 42.25,
                },
                heading: 64,
                speed: 25,
                speed_base: 25,
            },
            entities: vec![Entity {
                id: 0x1701234,
                act_index: 7,
                kind: EntityKind::Pc,
                name: Some("Other".into()),
                pos: Vec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                heading: 32,
                hp_pct: Some(80),
                bt_target_id: 0,
                face_target: 0,
                claim_id: 0,
                speed: 0,
                speed_base: 0,
                look: None,
                animation: 0,
                animationsub: 0,
                status: 0,
            }],
            party: vec![],
            chat: vec![ChatLine {
                channel: ChatChannel::Say,
                sender: "Other".into(),
                text: "hi".into(),
                server_ts: 1_700_000_000,
                local_seq: 0,
            }],
            diagnostics: Diagnostics {
                stage: Some(Stage::InZone),
                blowfish_status: Some(BlowfishStatus::Accepted),
                sync_in: Some(42),
                sync_out: Some(43),
                last_server_packet_age_ms: Some(123),
                map_server_addr: Some("127.0.0.1:54230".into()),
            },
            net_stats: NetStats {
                send_bps: 152,
                recv_bps: 539,
                send_health: 100,
                recv_health: 100,
            },
            current_goal: Some(ReactorGoal::Engaged {
                target_id: 0x99,
                attack_issued: true,
            }),
            last_reconnect: Some(ReconnectInfo {
                downtime_ms: 1234,
                at_unix_ms: 1_700_000_001_000,
            }),
            producer_monotonic_ms: 1_500,
            self_char_id: Some(0xCAFE_F00D),
            dialog: None,
            shop: None,
            status_icons: Vec::new(),
            status_icon_expiries: Vec::new(),
            ability_recasts: Vec::new(),
            logout_countdown: None,
            death_homepoint_secs: None,
            weather: None,
            equipped: [None; 16],
            spells_known: Vec::new(),
            job_abilities_known: Vec::new(),
            weaponskills_known: Vec::new(),
            pet_abilities_known: Vec::new(),
            key_items: Vec::new(),
            key_items_seen: Vec::new(),
            containers: Vec::new(),
            stats: None,
            bazaar: Vec::new(),
            play_time_s: 0,
            self_fishing: None,
            myroom: Some(MyRoom {
                model: 257,
                sub_map: 0,
            }),
            mh_2f_unlocked: None,
            emote_jobs: None,
            emote_chairs: None,
            check: None,
        }
    }

    #[test]
    fn self_in_mog_house_mirrors_producer_logic() {
        let mut snap = sample_snapshot();
        assert!(snap.self_in_mog_house(), "myroom cluster alone suffices");

        snap.myroom = None;
        assert!(!snap.self_in_mog_house(), "no myroom and empty party");

        snap.party = vec![PartyMember {
            id: 0xCAFE_F00D,
            act_index: 0,
            name: None,
            hp: 1,
            mp: 0,
            tp: 0,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 1,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: true,
        }];
        assert!(snap.self_in_mog_house(), "self party member flag suffices");

        snap.party[0].id = 0xDEAD_BEEF;
        assert!(
            !snap.self_in_mog_house(),
            "another member's flag must not count"
        );
    }

    #[test]
    fn targetability_rules() {
        let base = sample_snapshot().entities.remove(0);
        assert_eq!(base.kind, EntityKind::Pc);

        let live_pc = base.clone();
        assert!(live_pc.is_targetable() && live_pc.is_cycle_candidate());

        let dead_pc = Entity {
            hp_pct: Some(0),
            ..base.clone()
        };
        assert!(dead_pc.is_dead());
        assert!(
            dead_pc.is_targetable(),
            "dead PC stays targetable for Raise"
        );
        assert!(
            !dead_pc.is_cycle_candidate(),
            "no corpse cycles, even an ally's"
        );

        let dead_mob = Entity {
            kind: EntityKind::Mob,
            hp_pct: Some(0),
            ..base.clone()
        };
        assert!(!dead_mob.is_targetable() && !dead_mob.is_cycle_candidate());

        let live_mob = Entity {
            kind: EntityKind::Mob,
            hp_pct: Some(50),
            status: 1,
            ..base.clone()
        };
        assert!(live_mob.is_targetable() && live_mob.is_cycle_candidate());

        let other = Entity {
            kind: EntityKind::Other,
            ..base.clone()
        };
        assert!(!other.is_targetable());

        let npc_unknown_hp = Entity {
            kind: EntityKind::Npc,
            hp_pct: None,
            ..base.clone()
        };
        assert!(
            npc_unknown_hp.is_targetable(),
            "unknown-HP NPC stays targetable"
        );

        for status in [2u8, 3, 4, 6, 18, 20] {
            let hidden = Entity {
                kind: EntityKind::Mob,
                hp_pct: Some(50),
                status,
                ..base.clone()
            };
            assert!(
                !hidden.is_targetable(),
                "STATUS_TYPE {status} must not be targetable"
            );
        }
    }

    #[test]
    fn frame_snapshot_postcard_roundtrip() {
        let frame = Frame::Snapshot(Box::new(sample_snapshot()));
        let bytes = postcard::to_allocvec(&frame).expect("encode");
        let back: Frame = postcard::from_bytes(&bytes).expect("decode");
        match back {
            Frame::Snapshot(s) => {
                assert_eq!(s.stage, Stage::InZone);
                assert_eq!(s.char_name.as_deref(), Some("Sylvie"));
                assert_eq!(s.entities.len(), 1);
                assert_eq!(s.entities[0].id, 0x1701234);
                assert_eq!(s.chat[0].text, "hi");

                match s.current_goal {
                    Some(ReactorGoal::Engaged {
                        target_id,
                        attack_issued,
                    }) => {
                        assert_eq!(target_id, 0x99);
                        assert!(attack_issued);
                    }
                    other => panic!("goal: {other:?}"),
                }
                let rc = s.last_reconnect.expect("last_reconnect");
                assert_eq!(rc.downtime_ms, 1234);
                assert_eq!(s.producer_monotonic_ms, 1_500);
                assert_eq!(
                    s.myroom,
                    Some(MyRoom {
                        model: 257,
                        sub_map: 0
                    })
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn frame_event_postcard_roundtrip() {
        let frame = Frame::Event(ViewerEvent::TellReceived {
            from: "Friend".into(),
            text: "@cure".into(),
        });
        let bytes = postcard::to_allocvec(&frame).expect("encode");
        let back: Frame = postcard::from_bytes(&bytes).expect("decode");
        match back {
            Frame::Event(ViewerEvent::TellReceived { from, text }) => {
                assert_eq!(from, "Friend");
                assert_eq!(text, "@cure");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn client_frame_command_postcard_roundtrip() {
        let cf = ClientFrame::Command(ViewerCommand::Follow {
            target_id: 0x42,
            distance: 3.0,
        });
        let bytes = postcard::to_allocvec(&cf).expect("encode");
        let back: ClientFrame = postcard::from_bytes(&bytes).expect("decode");
        match back {
            ClientFrame::Command(ViewerCommand::Follow {
                target_id,
                distance,
            }) => {
                assert_eq!(target_id, 0x42);
                assert!((distance - 3.0).abs() < f32::EPSILON);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn viewer_command_action_surface_postcard_roundtrip() {
        let cmds = vec![
            ViewerCommand::Cast {
                spell_id: 0x101,
                target_id: 0xCAFE,
                target_index: 7,
                pos_x: 1.5,
                pos_y: 0.0,
                pos_z: -2.5,
            },
            ViewerCommand::Weaponskill {
                skill_id: 0xBEEF,
                target_id: 0xCAFE,
                target_index: 7,
            },
            ViewerCommand::JobAbility {
                ability_id: 0xABCD,
                target_id: 0,
                target_index: 0,
            },
            ViewerCommand::UseItem {
                container: 0,
                slot: 4,
                item_no: 4112,
                target_id: 0,
                target_index: 0,
            },
            ViewerCommand::BankWhenFull {
                threshold: 60,
                mog_house_zoneline: 0xDEAD_BEEF,
            },
        ];
        for c in cmds {
            let bytes = postcard::to_allocvec(&c).expect("encode");
            let back: ViewerCommand = postcard::from_bytes(&bytes).expect("decode");

            assert_eq!(format!("{c:?}"), format!("{back:?}"));
        }
    }

    #[test]
    fn frame_hello_json_debuggable() {
        let f = Frame::Hello {
            protocol_version: PROTOCOL_VERSION,
        };
        let s = serde_json::to_string(&f).unwrap();

        assert!(s.contains("\"Hello\""), "shape: {s}");
        let back: Frame = serde_json::from_str(&s).unwrap();
        match back {
            Frame::Hello { protocol_version } => {
                assert_eq!(protocol_version, PROTOCOL_VERSION)
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn from_lsb_known_ids_map_in_order() {
        use Weather::*;
        let expected = [
            None,
            Sunshine,
            Clouds,
            Fog,
            HotSpell,
            HeatWave,
            Rain,
            Squall,
            DustStorm,
            SandStorm,
            Wind,
            Gales,
            Snow,
            Blizzards,
            Thunder,
            Thunderstorms,
            Auroras,
            StellarGlare,
            Gloom,
            Darkness,
        ];
        for (n, &w) in expected.iter().enumerate() {
            assert_eq!(Weather::from_lsb(n as u16), w, "id {n}");
        }
    }

    #[test]
    fn from_lsb_unknown_ids_are_none() {
        // weather.h:46 unknown 0x14-0x27 set must not wrap onto real weathers.
        assert_eq!(Weather::from_lsb(20), Weather::None);
        assert_eq!(Weather::from_lsb(26), Weather::None);
        assert_eq!(Weather::from_lsb(39), Weather::None);
    }
}
