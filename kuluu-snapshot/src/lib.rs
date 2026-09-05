#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

// v23: SceneSnapshot.death_menu_offer — the durable s2c 0x0F9 Raise/Reraise or
// Tractor offer shown while dead. (Upstream's "v20"; renumbered on merge because our
// side had already spent 20-22 on zone_generation / untargetable / name_vis.)
// v22: Entity.name_vis is now Option<u8> — None until a General-block update carries
// it. The byte rides UPDATE_HP (entity_update.cpp:357/:408), not the Position block,
// so a POS-only 0x00E must not clobber the last known value with its zero-filled byte.
// v21: Entity.char_flags.untargetable — flags1 TargetOffFlag, the server's
// targetability authority (LSB m_flags FLAG_UNTARGETABLE for NPC/MOB, the explicit
// "Untargetable player" bit for PCs). namevis no longer gates targeting.
// v20: SceneSnapshot.zone_generation — a counter bumped on every zone change so the
// party frame's content key differs after a transition even when the roster is
// byte-identical to the previous zone (the fast-path race where the 0x0DD/0x0DF refill
// lands in the same poll as the ZoneChanged clear).
// v19: the cutscene channel — ViewerEvent::{CutsceneStarted,CutsceneCue,CutsceneEnded} plus
// CutsceneCue/CutsceneActor. The event VM's staging opcodes (actor motion, screen fade,
// camera lock, event-hide, mount) had no way across the boundary at all before this.
// v18: EntityLook::Door.door_id — the door's FourCC, which joins the entity to the MZB
// placement group and the zone-DAT routines that swing it (nothing else on the wire can).
// v17: ViewerEvent::Auction{MenuOpened,BidResult,SellResult,SellRefused,CancelResult,
// SearchFailed} — edge-triggered AH outcomes the HUD turns into retail chat echoes
// (the snapshot's AuctionUi carries only the settled state, not the transition).
// v16: SceneSnapshot.auction — the Auction House surface (counter open flag, browse
// catalog, price history, the 7 sales-status slots, fee quote, busy spinner).
// v15: Entity.mount + SceneSnapshot.self_mount — which mount is being ridden, which picks
// the mount's model. `animation`/`self_server_status` say only *that* someone is mounted.
// v14: the retail /check window's remaining panes — CheckResult.linkshell, SceneSnapshot
// .check_message (s2c 0x0CA), and SceneSnapshot.bazaar as a browsed-bazaar view (seller +
// per-slot rows with tax) instead of the old never-populated Vec<BazaarEntry>.
// v13: SceneSnapshot.chat_base_seq — the absolute history index of chat[0], so the viewer can
// merge its local toasts against a key that a full-snapshot resend does not renumber.
// v12: ViewerEvent::ActionStarted.animation — the BATTLE2 first-result animation index for
// every category, which is what keys the caster's effect DAT (the action id does not).
// v11: ChatLine.spans (per-substitution colouring — retail renders a drop line's item name
// green against the rest) and SceneSnapshot.treasure_pool (the 10 pool slots).
// v10: Entity.char_flags (0x0D/0x0E Flags1-3, for retail nameplate colour + icon markers) and
// PartyMember.party_no (GAttr.PartyNo, to tell an alliance-mate's claim from a party-mate's).
// v9: ViewerEvent::ActionStarted.result — optional (resolution, animation) pair (None for a
// truncated, result-less, or non-basic-attack BATTLE2 body).
// v8: SceneSnapshot.self_server_status (0x037 animation byte for self — the server's
// authoritative rest state, which CHAR_PC only carries for other players).
// v7: ViewerEvent::ActionStarted.{resolution, animation} (BATTLE2 first-result hit type +
// swing slot, for the melee reaction/swing routines).
// v6: ViewerEvent::ActionStarted.target_id (BATTLE2 primary target, for DAT attachType placement).
// v5: InventoryItem.charges_remaining + next_use_vana_ts (item recast/charges).
// v4: SceneSnapshot.delivery_box (dedicated delivery screen) + ViewerCommand::DeliveryBox
// (postcard frames are not self-describing, so any shape change bumps this).
pub const PROTOCOL_VERSION: u32 = 23;

/// Longest countdown `SceneSnapshot::status_icon_expiries` can carry. The
/// producer rejects anything beyond it as a corrupt 0x063 timestamp, and the HUD
/// reserves label width for the widest string inside the same bound, so the two
/// cannot drift into a countdown nothing has room to draw.
pub const MAX_STATUS_TIMER_SECS: u32 = 100 * 3600;

/// The one clock for `ability_recasts` math: local wall-clock Unix seconds.
/// The producer stamps expiries with it and every gate/display computes
/// remaining time against it — reading a different clock (e.g. the
/// server-anchored Vana'diel clock) lets the menu and the dispatch gate
/// disagree under client/server skew (kuluu-t815).
pub fn recast_now_unix() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
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

        /// `ffxi_proto::decode::DoorId` as its four raw bytes — the FourCC LSB
        /// writes into the door's 0x0E look. It names both the zone-DAT
        /// directory holding the door's `open`/`clos` routines and the MZB
        /// `BlockID` of the leaves those routines swing, so it is the only
        /// join from this entity to its geometry. `None` when the server sent
        /// an all-zero id (`DoorId::new`'s reject).
        #[serde(default)]
        door_id: Option<[u8; 4]>,
    },
    Transport {
        size: u16,
    },
}

/// Scene-side mirror of `ffxi_proto::decode::CharFlags` — the 0x0D/0x0E
/// `Flags1`/`Flags2`/`Flags3` bits the nameplate needs. The bit layout lives
/// with the decoder; this carries only the decoded values across the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CharFlags {
    pub monster: bool,
    pub lfg: bool,
    pub anonymous: bool,
    pub yell: bool,
    pub away: bool,
    pub play_online: bool,
    pub linkshell: bool,
    pub linkdead: bool,
    pub gm_level: u8,
    pub bazaar: bool,
    pub linkshell_color: [u8; 3],
    pub charm: bool,
    pub gm_icon: bool,
    pub auto_party: bool,
    pub trust: bool,
    pub lfg_master: bool,
    pub pet: bool,
    pub allegiance: u8,
    pub new_character: bool,
    pub mentor: bool,

    /// `Flags1.TargetOffFlag` (bit 19): the server's untargetable bit — LSB
    /// `m_flags & FLAG_UNTARGETABLE` for NPC/MOB, char_update's "Untargetable
    /// player" field for PCs. The targetability authority; see
    /// [`Entity::is_targetable`] and ffxi-proto's decode citation.
    #[serde(default)]
    pub untargetable: bool,
}

/// A mount being ridden. Retail draws the two arms from different model families
/// — the chocobo is a PC race config (one per colour), everything else an NPC
/// model — so the split is carried across the wire rather than re-derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mount {
    /// `MOUNT_CHOCOBO` or `MOUNT_NOBLE_CHOCOBO`, with the colour the server sent.
    Chocobo { colour: ChocoboColour },
    /// Any other `MOUNTTYPE`, carried verbatim.
    Other { mount_id: u8 },
}

impl Mount {
    /// Whether this mount's model comes from the PC race-config family rather
    /// than the NPC mount block.
    pub fn is_chocobo(self) -> bool {
        matches!(self, Mount::Chocobo { .. })
    }
}

/// Retail's five chocobo coat colours, each a separate race config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ChocoboColour {
    #[default]
    Yellow,
    Black,
    Blue,
    Red,
    Green,
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

    /// The mount this entity is riding, or `None` on foot. Resolved by the
    /// producer: the raw mount index stays set after a dismount, so only the
    /// animation byte can say whether it means anything.
    #[serde(default)]
    pub mount: Option<Mount>,

    #[serde(default)]
    pub status: u8,

    #[serde(default)]
    pub char_flags: CharFlags,

    /// entity_update byte 0x2B (LSB `namevis`; PosHead `flags3 >> 24`), written
    /// under UPDATE_HP — vendor/server/src/map/packets/entity_update.cpp:357/:408.
    /// `None` until the first General-block update carries it; treated as visible,
    /// matching the server's VIS_NONE default (baseentity.cpp:45). LSB NAMEVIS
    /// (vendor/server/src/map/entities/baseentity.h): 0x01 icon, 0x08 hide-name,
    /// 0x80 ghost-phase — the other bits in the data are render-phase flags on real
    /// NPCs (Survival Guides carry 0x20), so only 0x08 suppresses anything.
    #[serde(default)]
    pub name_vis: Option<u8>,
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

    /// Retail-hidden helper NPC: VIS_HIDE_NAME set — mannequins, "blank"
    /// cutscene actors. vendor/server/src/map/entities/baseentity.cpp:159
    /// `IsNameHidden() = namevis & FLAG_HIDE_NAME` (0x08); the NAMEVIS enum
    /// defines only 0x01/0x08/0x80, so the other bits are render-phase flags,
    /// not name suppression. Suppresses the nameplate only — never targeting.
    pub fn name_hidden(&self) -> bool {
        self.name_vis.is_some_and(|v| v & 0x08 != 0)
    }

    // Blacklist (not whitelist) so an undecoded byte fails open, staying targetable.
    fn status_selectable(&self) -> bool {
        use status_type::*;
        !matches!(
            self.status,
            DISAPPEAR | INVISIBLE | STATUS_4 | CUTSCENE_ONLY | STATUS_18 | SHUTDOWN
        )
    }

    /// A server-side door NPC. Doors classify to `EntityKind::Other` but are
    /// interactable: retail sends a Talk (0x01A, action 0x00) on the door's
    /// act_index and the door's onTrigger lua drives open/confirm/zone-change.
    /// LSB gates doors on `look.size == 0x02`
    /// (vendor/server/src/map/packets/c2s/0x01a_action.cpp:213); size 3/4 decode
    /// to `Transport` (elevators/airships), which stay non-interactable.
    pub fn is_door(&self) -> bool {
        matches!(self.look, Some(EntityLook::Door { .. }))
    }

    /// Selectable by click / `<t>`. Dead players stay selectable so a healer can
    /// target them to Raise; dead mobs/NPCs do not. `Other` entities are not
    /// selectable except doors, whose Talk interaction is the retail door flow.
    /// Targetability authority is the server's untargetable bit (flags1
    /// TargetOffFlag = LSB m_flags FLAG_UNTARGETABLE for NPC/MOB) — namevis
    /// never gates targeting upstream.
    pub fn is_targetable(&self) -> bool {
        if !self.status_selectable() {
            return false;
        }
        if self.char_flags.untargetable {
            return false;
        }
        if matches!(self.kind, EntityKind::Other) && !self.is_door() {
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

    /// Chat kind 8 MESSAGE_EMOTION: canned-emote lines (caster name already
    /// embedded in `text`, `sender` empty) and free-form /em (`sender` set,
    /// `text` is the raw emote body).
    Emote,
}

/// Which retail colour a run of a chat line takes. Retail renders some
/// substitutions apart from the text around them — the item name in
/// "You find a [lizard tail] on the Rock Lizard." is green against the rest
/// (`.agents/skills/retail-observe/references/treasure-pool-chat.md`).
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

/// Whether the local player has acted on a pool item (s2c 0x0D2 `Entry`).
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreasureEntry {
    #[default]
    None,
    Passed,
    Lotted,
}

/// One occupied treasure-pool slot, as the pool panel renders it.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TreasurePoolSlot {
    pub slot: u8,
    pub item_id: u16,
    pub item_name: String,
    pub count: u32,
    pub dropper: String,
    pub own_entry: TreasureEntry,
    pub own_lot: Option<u16>,
    pub winner: Option<String>,
    pub winner_lot: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatLine {
    pub channel: ChatChannel,
    pub sender: String,
    pub text: String,
    pub server_ts: u32,

    /// Viewer-local merge key, meaningful only on a local toast: the number of
    /// session chat lines that had already arrived when the toast was pushed.
    /// Server lines key off [`SceneSnapshot::chat_base_seq`] instead.
    #[serde(default)]
    pub local_seq: u64,

    /// Per-substitution colouring, for the lines retail renders multicoloured.
    /// Empty means the whole line takes the channel colour; when set, the
    /// concatenated span text equals `text`.
    #[serde(default)]
    pub spans: Vec<ChatSpan>,
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
    /// "no party"). vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp:40.
    #[serde(default)]
    pub party_no: u8,

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

    /// Monotonically increasing counter, bumped on every zone change. Forces
    /// the party-frame content key to differ after a zone transition even when
    /// the party data is byte-identical.
    #[serde(default)]
    pub zone_generation: u64,

    pub chat: Vec<ChatLine>,

    /// Absolute index of `chat[0]` in the session's whole chat history, so
    /// `chat_base_seq + i` is a stable id for `chat[i]` that survives both a
    /// full-snapshot resend and `CHAT_HISTORY_CAP` draining. The viewer merges
    /// its own local toasts against this; a position-derived key cannot, because
    /// every resend renumbers it (kuluu-zvc3).
    #[serde(default)]
    pub chat_base_seq: u64,

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

    /// `Some` while a delivery box is open (drives the dedicated delivery screen).
    #[serde(default)]
    pub delivery_box: Option<DeliveryBoxState>,

    /// Occupied treasure-pool slots, newest drop last. Empty when the pool is.
    #[serde(default)]
    pub treasure_pool: Vec<TreasurePoolSlot>,

    #[serde(default)]
    pub status_icons: Vec<u16>,

    /// Absolute Unix expiry per `status_icons` entry, 0 for a permanent effect.
    /// Never further out than [`MAX_STATUS_TIMER_SECS`] — the producer treats a
    /// longer remaining time as a corrupt timestamp and drops it to 0.
    #[serde(default)]
    pub status_icon_expiries: Vec<u32>,

    /// (recast group id, absolute expiry) pairs; expiries are local-clock Unix
    /// seconds stamped with [`recast_now_unix`], which all readers must use.
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

    /// The bazaar currently being browsed (View Wares from the Check window).
    #[serde(default)]
    pub bazaar: Option<BazaarView>,

    /// The Auction House surface (counter menu, browse catalog, price history,
    /// sales status). `auction.open` gates the HUD; current gil is read from
    /// `containers` (LOC_INVENTORY slot 0), not duplicated here.
    #[serde(default)]
    pub auction: AuctionUi,

    #[serde(default)]
    pub play_time_s: u64,

    /// Self fishing state, present while the player is fishing. Drives the self pose and
    /// the mini-game HUD.
    #[serde(default)]
    pub self_fishing: Option<SelfFishing>,

    /// The server's animation byte for self, from 0x037 CHAR_STATUS
    /// (`vendor/server/src/map/packets/char_status.cpp:221` — `PChar->animation`).
    /// Authoritative for the rest stance: CHAR_PC carries `Entity::animation` for
    /// other players, but self's own state only arrives here.
    #[serde(default)]
    pub self_server_status: u8,

    /// The mount the player is riding, or `None` on foot. Self never appears in
    /// the CHAR_PC stream that carries `Entity::mount` for other players, so this
    /// comes from 0x037 instead.
    #[serde(default)]
    pub self_mount: Option<Mount>,

    /// Self casting/action state, present while an issued spell/ability is in
    /// flight. Drives the Enhanced cast bar. Optimistic on send, reconciled by
    /// the server's BATTLE2 start/finish/interrupt.
    #[serde(default)]
    pub self_casting: Option<SelfCasting>,

    /// `Some` while the server has the player inside a Mog House (same zone_id as
    /// the surrounding city); the renderer must re-key zone resources on it.
    #[serde(default)]
    pub myroom: Option<MyRoom>,

    /// Whether the Mog House 2nd floor is unlocked (0x055 char sync); gates the
    /// Mog Safe 2 bag — the server drops moves into it without profile.mhflag
    /// bit 0x20 (0x029_item_move.cpp validContainers). `None` = not yet known.
    #[serde(default)]
    pub mh_2f_unlocked: Option<bool>,

    /// `SubMapNumber` from 0x00A LOGIN (`PChar->loc.boundary`): the sub-area
    /// interior the server has the character standing in at zone-in, which the
    /// renderer's sub-area latch seeds from. `None` until a login lands.
    #[serde(default)]
    pub sub_area: Option<u16>,

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

    /// s2c 0x0CA answer to the same /check: the target's bazaar message, keyed
    /// only by their name (the packet carries no target id).
    #[serde(default)]
    pub check_message: Option<CheckMessage>,

    /// Server-driven wide-scan (tracking) list and currently tracked target.
    /// Populated from s2c 0x0F4/0x0F5/0x0F6; the viewer renders it without
    /// touching `SessionState` (see `ffxi_proto::map::tracking`).
    #[serde(default)]
    pub widescan: WidescanList,

    /// Server-offered alternative to returning to the home point while dead.
    /// `None` is the ordinary home-point-only menu.
    #[serde(default)]
    pub death_menu_offer: Option<DeathMenuOffer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeathMenuOffer {
    Raise,
    Tractor,
}

/// Mirror of `kuluu`'s wide-scan model across the wire boundary. Entries
/// arrive between s2c 0x0F6 ListStart/ListEnd frames; `tracked` follows 0x0F5.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WidescanList {
    pub entries: Vec<WidescanEntry>,
    pub tracked: Option<WidescanTracked>,
}

/// One wide-scan list row. Mirrors `ffxi_proto::decode::WidescanEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidescanEntry {
    pub act_index: u16,
    pub level: u8,
    /// Marker category: 0 = char, 1 = npc, 2 = mob (0x0f4_tracking_list.cpp).
    pub kind: u8,
    /// Entity minus self position, server units.
    pub rel_x: i16,
    pub rel_z: i16,
    /// Server sName, or the session's zone NPC-name DAT enrichment when the
    /// server sends it empty; the viewer falls back to the local entity name
    /// keyed on `act_index`.
    pub name: String,
}

/// The currently tracked entity's absolute position. Mirrors
/// `ffxi_proto::decode::WidescanPos` (raw server coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WidescanTracked {
    pub act_index: u16,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// `equipped` is indexed by SAVE_EQUIP_KIND slot id (0 = Main .. 15 = Back);
/// jobs are zero while the target is /anon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub target_id: u32,
    pub equipped: [Option<u16>; 16],
    pub main_job: u8,
    pub sub_job: u8,
    pub main_job_lv: u8,
    pub sub_job_lv: u8,
    pub master_lv: u8,
    /// Equipped linkshell's name; empty when the target wears no pearl.
    #[serde(default)]
    pub linkshell: String,
}

/// s2c 0x0CA INSPECT_MESSAGE: the checked PC's bazaar/seek message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckMessage {
    pub name: String,
    pub message: String,
}

impl SceneSnapshot {
    /// The mount `entity` is riding. Self is in `entities` but never receives the
    /// CHAR_PC stream that fills `Entity::mount`, so its mount arrives separately
    /// and has to be folded back in here — every consumer wants the same answer
    /// for self and for everyone else.
    pub fn mount_of(&self, entity: &Entity) -> Option<Mount> {
        if self.self_char_id == Some(entity.id) {
            self.self_mount
        } else {
            entity.mount
        }
    }

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

/// Self casting/action view for the renderer/HUD cast bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelfCasting {
    /// Display name of the spell/ability being performed (already resolved).
    pub name: String,
    /// Milliseconds elapsed since the action started.
    pub elapsed_ms: u32,
    /// Total expected duration in milliseconds (spell castTime, or the JA/WS/RA
    /// animation lock). The bar fill fraction is `elapsed_ms / total_ms`.
    pub total_ms: u32,
    /// Set once the action was interrupted (moved mid-cast, paralyzed, …); the
    /// bar flashes then clears.
    pub interrupted: bool,
}

/// Which "something caught the hook" line the server sent; retail labels the
/// mini-game bar off it (research/xim FishHppUi.kt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FishSize {
    Small,
    Large,
}

impl FishSize {
    pub fn label(self) -> &'static str {
        match self {
            FishSize::Small => "Small Fish",
            FishSize::Large => "Large Fish",
        }
    }
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
    /// Hooked-fish size, once the server's hook message has landed.
    #[serde(default)]
    pub size: Option<FishSize>,
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

/// Mirror of `kuluu`'s browsed-bazaar model (s2c 0x105 rows keyed by the
/// seller's inventory slot).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BazaarView {
    pub seller_id: u32,
    pub seller_name: String,
    pub items: Vec<BazaarEntry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BazaarEntry {
    /// Seller-side inventory slot: the id a purchase names.
    pub index: u8,
    pub item_no: u16,
    pub quantity: u32,
    /// Asking price per unit, before tax.
    pub price: u32,
    /// Zone tax in hundredths of a percent; [`BazaarEntry::total_price`] applies it.
    pub tax_rate: u16,
}

impl BazaarEntry {
    /// Gil charged for `quantity` units, tax included. Mirrors
    /// `kuluu::state::BazaarItem::total_price` (LSB
    /// vendor/server/src/map/packets/c2s/0x106_bazaar_buy.cpp:103).
    pub fn total_price(&self, quantity: u32) -> u32 {
        const TAX_DIVISOR: u64 = 10_000;
        let base = u64::from(self.price) * u64::from(quantity);
        u32::try_from(base + u64::from(self.tax_rate) * base / TAX_DIVISOR).unwrap_or(u32::MAX)
    }
}

/// Sales-status slot count. Mirrors `ffxi_proto::decode::AUCTION_SLOT_COUNT`
/// (GP_SERV_COMMAND_AUC carries 7 Parcel slots); a producer-side guard test
/// pins the two together.
pub const AH_SALES_SLOT_COUNT: usize = 7;

/// Mirror of `kuluu`'s `AuctionState`. `open` is set by the s2c Open
/// push and only clears on zone change (the counter NPC is zone-local).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuctionUi {
    pub open: bool,
    pub browse: Option<AhCatalogView>,
    pub history: Option<AhHistoryView>,
    pub sales_status: [Option<AhSaleStatus>; AH_SALES_SLOT_COUNT],
    /// Last AskCommit fee quote, awaiting the sell confirm.
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

/// One category's full catalog (all search-server pages merged).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhCatalogView {
    pub category: u8,
    pub total: u16,
    pub listings: Vec<AhListingView>,
}

/// One catalog row — open-listing counts (the retail bracketed `[N]` stock
/// numbers), never prices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhListingView {
    pub item_id: u16,
    /// Singles currently up for sale; 0 = none listed.
    pub singles_for_sale: u32,
    /// Stacks currently up for sale; `None` = item is not stackable.
    pub stacks_for_sale: Option<u32>,
}

/// Price history for one item in one form (single or stack).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhHistoryView {
    pub item_id: u16,
    pub stack: bool,
    /// Count of open listings of the requested form; not a price.
    pub open_listings: u32,
    pub category: u16,
    pub sales: Vec<AhSaleView>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhSaleView {
    pub price: u32,
    pub sell_date: u32,
    pub seller: String,
    pub buyer: String,
}

/// One populated sales-status slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhSaleStatus {
    pub stat: u8,
    pub item_no: u16,
    /// 1 for a single, the stack size for a stack.
    pub quantity: u8,
    pub price: u32,
    pub timestamp: u32,
}

/// The AskCommit fee quote plus the asking price the session sent (the quote
/// does not echo it). Drives the retail fee/placement Yes/No confirms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AhFeeQuote {
    pub fee: u32,
    pub inventory_slot: u8,
    pub item_no: u16,
    pub stack: bool,
    pub asking_price: u32,
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
    /// Current charges of a charged (usable/enchanted) item; `None` for
    /// non-charged items. From item extdata
    /// (vendor/server/src/map/items/exdata/timer_info.h:31-32, memcpy'd at
    /// 0x020_item_attr.cpp:43).
    #[serde(default)]
    pub charges_remaining: Option<u8>,
    /// Absolute Vana'diel next-use timestamp (Earth seconds since the vanadiel
    /// epoch), `None` for non-charged items. Not zeroed on the ready path — LSB
    /// only writes it on cooldown (0x020_item_attr.cpp:57-68) — so gate on
    /// `ts > now`, not `ts == 0`.
    #[serde(default)]
    pub next_use_vana_ts: Option<u32>,
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

    /// Grid presentation metadata for a choice frame (delivery-box 2x4 slot
    /// grid). Cells are row-major; each active cell maps back to an index in
    /// `choices`, so answering works identically to a plain list frame.
    #[serde(default)]
    pub grid: Option<DialogGrid>,

    /// Server customMenu (GMPROMPT/`_CUSTOM_MENU`) prompt: the viewer answers
    /// with `AgentCommand::CustomMenuRespond` instead of an `EndEventChoice`.
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

    /// True once an outgoing item has been dispatched to the server.
    pub sent: bool,
}

/// Which delivery box the server has open. Mirrors the client-side
/// `DeliveryBoxNo` and LSB `GP_CLI_COMMAND_PBX_BOXNO` (Outgoing = send,
/// Incoming = receive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryBoxNo {
    Incoming,
    #[default]
    Outgoing,
}

/// Resolution state of the outgoing recipient name (send box only). `Ok`'s
/// `same_account` mirrors LSB ResParam1 (recipient shares the sender's
/// account), which unlocks account-bound item delivery.
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

/// One occupied delivery box slot. `counterpart` is the sender (Incoming) or
/// recipient (Outgoing); `stat` is the raw GP_POST_BOX_STATE Stat byte
/// (`ffxi_proto::map::pbx::stat`), which the viewer reads for staged/sent dimming.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverySlot {
    pub item_no: u16,
    pub quantity: u32,
    #[serde(default)]
    pub counterpart: Option<String>,
    pub stat: u32,
}

/// The dedicated delivery box screen model. `Some` on `SceneSnapshot` ⇒ the box
/// is open and the viewer renders the delivery screen. Inventory list and
/// current gil are read from `containers` (gil = LOC_INVENTORY slot 0), not
/// duplicated here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryBoxState {
    pub box_no: DeliveryBoxNo,
    /// `pbx::SLOT_COUNT` (8) entries, row-major over the retail 2x4 grid.
    pub slots: Vec<Option<DeliverySlot>>,
    /// Items still queued beyond the 8 visible slots (last Check answer).
    pub queued: u8,
    /// Outgoing: the typed/locked recipient name.
    #[serde(default)]
    pub recipient: Option<String>,
    /// Outgoing: recipient name resolution state.
    #[serde(default)]
    pub recipient_status: RecipientStatus,
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

/// [`ViewerEvent::ZoneChanged::to`] for the half of a zone change that only
/// tears the connection down: the server has said "reconnect over there" and the
/// destination is not known until the new map session hands it over. No real
/// zone is 0, so it cannot collide with an arrival.
pub const ZONE_UNKNOWN: u16 = 0;

/// A four-character scheduler/action key in file byte order. The tag values
/// themselves are `ffxi_event`'s (`SCHEDULER_TAG_FADE_OUT`, …) — this crate
/// only carries them, so it never names one.
pub type FourCc = [u8; 4];

/// Which entity a [`CutsceneCue`] names. The event VM's own operand is an
/// unresolved `ActorLookup`; the producer resolves it against the running
/// event's entity before it crosses this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutsceneActor {
    LocalPlayer,
    Entity { server_id: u32 },
}

/// One staging effect the running event script asked for, in execution order.
/// Scoped to the event session: every one of these is undone at
/// [`ViewerEvent::CutsceneEnded`], because the bytecode routinely never undoes
/// it itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CutsceneCue {
    /// Play action `key` on `actor`, with `partner` as the action's partner.
    ActorMotion {
        actor: CutsceneActor,
        partner: CutsceneActor,
        key: FourCc,
    },
    /// Run scheduler `tag` out of scheduler DAT `dat_id` over the two actors.
    /// `duration` 0 means "play the DAT-authored timing verbatim".
    Scheduler {
        dat_id: u32,
        actor: CutsceneActor,
        partner: CutsceneActor,
        tag: FourCc,
        duration: u16,
    },
    /// Set/clear the target's event-hide render flag. The flag is inert once
    /// the event session ends — retail stops consulting it rather than
    /// clearing it.
    ActorHide { target: CutsceneActor, hide: bool },
    /// Take camera control away from the player, or give it back.
    CameraLock { lock: bool },
    /// Put the target on or off a mount. `status_event` is the `GameStatus`
    /// value the script writes; `mount_id` is carried only by the non-chocobo
    /// mount cases.
    Mount {
        target: CutsceneActor,
        status_event: u8,
        mount_id: Option<u16>,
    },
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
        target_id: Option<u32>,
        /// First result's `resolution` (vendor/server enums/action/resolution.h) and
        /// `animation` (attack.h AttackAnimation) bits; only a `CATEGORY_BASIC_ATTACK` body
        /// carries them, absent otherwise.
        result: Option<(u8, u16)>,
        /// First result's raw `animation` index, for every category — the file-table key of
        /// the caster's effect DAT. Absent on a result-less or truncated body.
        animation: Option<u16>,
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

    /// s2c 0x04C Open: the AH counter menu opened. Edge-triggered (the
    /// snapshot's `AuctionUi::open` stays true until zone change, so a repeat
    /// Open at the counter is only visible here).
    AuctionMenuOpened,

    /// s2c 0x04C Bid echo; `ok` on Result 1, else the LSB message code drives
    /// the retail "You were unable to buy the <item> for <N> gil." echo.
    AuctionBidResult {
        ok: bool,
        item_no: u16,
        price: u32,
        quantity: u32,
    },

    /// s2c 0x04C LotIn verdict ("Merchandise placed on auction." on ok).
    AuctionSellResult {
        ok: bool,
    },

    /// s2c 0x04C AskCommit rejection (no fee quote); `result` is LSB's message
    /// code (197 = auctionutils SellingItems reject, e.g. a partial stack).
    AuctionSellRefused {
        result: u8,
    },

    /// s2c 0x04C LotCancel verdict for sales-status `slot`.
    AuctionCancelResult {
        slot: u8,
        ok: bool,
    },

    /// A search-server round trip (browse/history) failed.
    AuctionSearchFailed {
        message: String,
    },

    /// An event session opened. Everything a [`CutsceneCue`] changes is scoped
    /// to the span between this and [`ViewerEvent::CutsceneEnded`].
    CutsceneStarted {
        event_id: u32,
    },

    /// One staging cue from the running event script.
    Cutscene {
        cue: CutsceneCue,
    },

    /// The event session closed. Every scoped change reverts here, whether or
    /// not the script undid it: retail's de facto teardown for an unpaired
    /// camera lock is the zone change, so the producer sends this on every
    /// exit (script end, cancel, watchdog release, zone change, disconnect).
    CutsceneEnded,
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

    /// Delivery box interaction from the dedicated screen. Maps 1:1 to the
    /// session's `AgentCommand::DeliveryBox` / take chain (see `relay.rs`).
    DeliveryBox {
        op: DeliveryOp,
    },
}

/// Viewer-issued delivery box operations. A thinner vocabulary than the
/// session's `DeliveryBoxOp`: the recipient is pulled from the session's locked
/// name (never re-sent by the viewer), and Open/Take fan out to the right
/// per-box command server-side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DeliveryOp {
    /// DeliOpen (Outgoing) / PostOpen (Incoming).
    Open { box_no: DeliveryBoxNo },
    /// PostClose the open box.
    Close,
    /// Verify a recipient name before staging (Outgoing).
    Query { recipient: String },
    /// Stage `quantity` of LOC_INVENTORY `inventory_slot` (0 = gil) into outbox
    /// `slot`. Recipient comes from the locked name server-side.
    Set {
        slot: u8,
        inventory_slot: u8,
        quantity: u32,
    },
    /// Dispatch the staged item in outbox `slot`.
    Send { slot: u8 },
    /// Cancel a staged (take back) or dispatched item in outbox `slot`.
    CancelSlot { slot: u8 },
    /// Take an incoming parcel in inbox `slot` (session runs Accept→Get).
    Take { slot: u8 },
    /// Return an incoming parcel in inbox `slot` to its sender.
    Reject { slot: u8 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientFrame {
    Hello { protocol_version: u32 },
    Command(ViewerCommand),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_auction() -> AuctionUi {
        let mut sales_status = [None; AH_SALES_SLOT_COUNT];
        sales_status[2] = Some(AhSaleStatus {
            stat: 1,
            item_no: 4570,
            quantity: 12,
            price: 1_180,
            timestamp: 1_700_000_100,
        });
        AuctionUi {
            open: true,
            browse: Some(AhCatalogView {
                category: 7,
                total: 2,
                listings: vec![
                    AhListingView {
                        item_id: 4570,
                        singles_for_sale: 14,
                        stacks_for_sale: Some(3),
                    },
                    AhListingView {
                        item_id: 17440,
                        singles_for_sale: 0,
                        stacks_for_sale: None,
                    },
                ],
            }),
            history: Some(AhHistoryView {
                item_id: 4570,
                stack: true,
                open_listings: 3,
                category: 7,
                sales: vec![AhSaleView {
                    price: 1_180,
                    sell_date: 1_700_000_000,
                    seller: "Aliya".into(),
                    buyer: "Sylvie".into(),
                }],
            }),
            sales_status,
            fee_quote: Some(AhFeeQuote {
                fee: 9,
                inventory_slot: 5,
                item_no: 4570,
                stack: true,
                asking_price: 1_180,
            }),
            busy: Some(AuctionBusy::Downloading),
        }
    }

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
                mount: None,
                status: 0,
                char_flags: CharFlags::default(),
                name_vis: None,
            }],
            party: vec![],
            zone_generation: 7,
            chat: vec![ChatLine {
                channel: ChatChannel::Say,
                sender: "Other".into(),
                text: "hi".into(),
                server_ts: 1_700_000_000,
                local_seq: 0,
                spans: Vec::new(),
            }],
            chat_base_seq: 0,
            treasure_pool: vec![],
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
            delivery_box: None,
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
            bazaar: None,
            auction: sample_auction(),
            play_time_s: 0,
            self_fishing: None,
            self_server_status: 0,
            self_mount: None,
            self_casting: None,
            myroom: Some(MyRoom {
                model: 257,
                sub_map: 0,
            }),
            mh_2f_unlocked: None,
            sub_area: None,
            emote_jobs: None,
            emote_chairs: None,
            check: None,
            check_message: None,
            widescan: WidescanList::default(),
            death_menu_offer: None,
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
            party_no: 0,
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
            look: None,
            ..base.clone()
        };
        assert!(!other.is_targetable());

        let door = Entity {
            kind: EntityKind::Other,
            look: Some(EntityLook::Door {
                size: 2,
                door_id: None,
            }),
            ..base.clone()
        };
        assert!(
            door.is_targetable() && door.is_cycle_candidate(),
            "doors (Other + Door look) are interactable targets"
        );

        let transport = Entity {
            kind: EntityKind::Other,
            look: Some(EntityLook::Transport { size: 3 }),
            ..base.clone()
        };
        assert!(
            !transport.is_targetable(),
            "transports/elevators stay non-targetable"
        );

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
                assert_eq!(s.auction, sample_auction());
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

    /// Postcard frames are not self-describing, so a cue that works over the
    /// in-process bridge has to survive the relay's encode/decode too.
    #[test]
    fn a_cutscene_cue_survives_the_postcard_relay() {
        let cues = [
            CutsceneCue::Scheduler {
                dat_id: 30904,
                actor: CutsceneActor::LocalPlayer,
                partner: CutsceneActor::Entity {
                    server_id: 0x010E_602F,
                },
                tag: *b"fdi0",
                duration: 0,
            },
            CutsceneCue::CameraLock { lock: true },
            CutsceneCue::ActorHide {
                target: CutsceneActor::Entity {
                    server_id: 0x010E_6032,
                },
                hide: true,
            },
            CutsceneCue::Mount {
                target: CutsceneActor::LocalPlayer,
                status_event: 85,
                mount_id: Some(4),
            },
            CutsceneCue::ActorMotion {
                actor: CutsceneActor::Entity { server_id: 1 },
                partner: CutsceneActor::LocalPlayer,
                key: *b"kue0",
            },
        ];
        for cue in cues {
            let bytes =
                postcard::to_stdvec(&Frame::Event(ViewerEvent::Cutscene { cue })).expect("encode");
            let back: Frame = postcard::from_bytes(&bytes).expect("decode");
            let Frame::Event(ViewerEvent::Cutscene { cue: back }) = back else {
                panic!("frame kind changed across postcard: {back:?}");
            };
            assert_eq!(back, cue);
        }
    }

    /// SceneSnapshot is an EXTENSION SURFACE: the relay publishes it to
    /// external consumers (kuluu-mcp, the wasm viewer, future addons), so it
    /// evolves additive-only. This pin makes any field rename/removal (a
    /// breaking change for consumers) and any addition (fine, but must be
    /// deliberate) fail here first, so the diff shows the contract change.
    #[test]
    fn snapshot_top_level_fields_are_pinned() {
        let v = serde_json::to_value(SceneSnapshot::default()).unwrap();
        let mut got: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        got.sort();
        let mut want: Vec<&str> = vec![
            "stage",
            "char_name",
            "zone_id",
            "self_pos",
            "entities",
            "party",
            "zone_generation",
            "chat",
            "chat_base_seq",
            "diagnostics",
            "net_stats",
            "current_goal",
            "last_reconnect",
            "producer_monotonic_ms",
            "self_char_id",
            "dialog",
            "shop",
            "delivery_box",
            "treasure_pool",
            "status_icons",
            "status_icon_expiries",
            "ability_recasts",
            "logout_countdown",
            "death_homepoint_secs",
            "weather",
            "equipped",
            "spells_known",
            "job_abilities_known",
            "weaponskills_known",
            "pet_abilities_known",
            "key_items",
            "key_items_seen",
            "containers",
            "stats",
            "bazaar",
            "auction",
            "play_time_s",
            "self_fishing",
            "self_server_status",
            "self_mount",
            "self_casting",
            "myroom",
            "mh_2f_unlocked",
            "sub_area",
            "emote_jobs",
            "emote_chairs",
            "check",
            "check_message",
            "widescan",
            "death_menu_offer",
        ];
        want.sort();
        assert_eq!(got, want, "SceneSnapshot fields changed: additive-only, update this pin deliberately and rebuild relay consumers together");
    }

    const SNAPSHOT_DEFAULT_POSTCARD_HEX: &str = "0000000000000000000000000000000019190000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

    /// Postcard is positional, not self-describing: field ORDER and TYPES are
    /// the wire format. Any reorder/retype (and any append) changes these
    /// bytes; consumers built from the same commit stay in sync, but the
    /// change must be deliberate.
    #[test]
    fn snapshot_default_postcard_bytes_are_pinned() {
        let bytes = postcard::to_allocvec(&SceneSnapshot::default()).unwrap();
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex, SNAPSHOT_DEFAULT_POSTCARD_HEX,
            "SceneSnapshot postcard encoding changed: update the pin deliberately and rebuild relay consumers together"
        );
    }
}
