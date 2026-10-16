#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

// v45: CutsceneCue::ClockHold gains `minute` and `day_from_epoch` (0xA9
// SET_CLOCK_DATE jumps the whole date to Vana day 7*work[1] at 00:30; 0xC9
// ENABLE_TIMER releases the hold), so the 0x77/0x78 hour-only shape now carries
// minute 0 and no day (research/XiEvents/OpCodes/0x00A9.md, 0x00C9.md,
// 0x0077.md, 0x0078.md).
// v44: CutsceneCue::Transpar (0x6C) - the target's alpha fade to the authored
// byte over the authored frame count, the first actor-colour drive on the cue
// channel (research/XiEvents/OpCodes/0x006C.md).
// v43: CutsceneCue::PlayerControl (0x20) - the script's write of retail's
// CliEventUcFlag, so an event that releases the player mid-script (the flag's 0) stops the
// event-wide pin instead of holding it to EVENT_END
// (research/XiEvents/OpCodes/0x0020.md).
// v42: ViewerEvent::Knockbacks - every target result of one 0x028 that landed with a
// knockback level (GP_SERV_COMMAND_BATTLE2::pack), so the client can shove the victims when
// the skill routine's knockback stage fires; ActionStarted.outcome carries the first
// target's only.
// v41: EntityLook::Transport.travel_secs - the lift's seconds between floors from the 0x0E
// name block, the fallback for a shaft whose zone DAT routine states no travel of its own.
// v40: InventoryItem.unselectable - the NoSelect lock an in-flight item use holds, which is the
// only word the client gets that the item is spent until the use resolves seconds later.
// v39: CutsceneCue::ActorMove.speed is the raw 0x32 MainSpeed operand (i32,
// scaled by ffxi_event EVENT_SPEED_SCALE at the consumer) instead of yalms/sec
// as f32, which restores Eq on CutsceneCue for keyed hold tables.
// v38: CutsceneCue::HudHide (0x67/0x68) + CutsceneCue::ClockHold (0x77/0x78) and
// ViewerEvent::{MapOpen, MapMarkerPlaced, MapClosed} (event script 0xC8/0x8B/0x8A).
// v37: SceneSnapshot.self_pet_targid - the owner-only 0x068 PetSync targid, which
// PET-flagged ability targeting needs to offer the caster's own pet.
// v36: DialogState.{cancel_armed, speaker_index, contains_item} - retail's
// CliEventCancelFlag, the per-frame speaker's target index, and whether the line
// carried an item / key-item marker before substitution.
// v35: CutsceneCue::ZoneScheduler.zone_id - the 0x2D zone scene now carries the
// current zone so the host resolves its key out of the zone's own model DAT (with
// the entrance/instance partner and non-model carriers as fallbacks) instead of the
// global title-screen scene DAT.
// v34: CutsceneCue::EntityName (0xB5 case 0 display-name change, fed by the
// s2c 0x005D PENDINGSTR table via 0xB4 case 1).
// v33: CutsceneCue::ExtScheduler.motion - the 0x66 Tpc package now carries its two
// container file ids (A + the CIB-waist-selected B) instead of one flat id; None is the
// out-of-range package, which loads nothing.
// v32: CutsceneCue::ZoneScheduler - the 0x2D/0x54 zone scene routine out of the global
// scene DAT (ROM/0/23.DAT), whose camera routes drive the operator camera.
// v31: CutsceneCue::ExtScheduler (the 0x5B/0x66 motion-resource cue) and the
// actor cues ActorMove / ActorPlace / ActorFace / ActorLookAt / ActorStopAction
// that a REQSET-spawned NPC script emits.
// v30: ViewerEvent::ActionStarted.outcome - the first result block as one typed
// Option<ResultOutcome> (resolution + info bits + hitDistortion + knockback, ffxi-proto enums
// from the pinned vendor/server headers) instead of four parallel u8 fields that spelled "no
// result block" as zero. None means no result block was read; resolution 0 is Hit, so absence
// must not be a value.
// v29: ViewerEvent::ActionStarted.{info, hit_distortion, knockback, kind} - the first
// result's per-result outcome bits packed by BATTLE2 (s2c 0x028): Defeated/CriticalHit
// flags, the hit-distortion level and the knockback level that drive the victim's reaction
// routine.
// v28: InventoryItem.use_delay_end_vana_ts + ready (enchanted-item equip delay).
// v27: CharFlags.graph_size - Flags1.GraphSize, the server's per-entity size class.
// It indexes the model's four authored CIB scales, so without it every entity
// renders at the model's index-0 size and mob size variation is lost.
// v26: ViewerEvent::ActionStarted.outcome - the first result's (info, hitDistortion,
// knockback) bits (GP_SERV_COMMAND_BATTLE2::pack) that drive the victim's reaction routine.
// v25: ViewerEvent::TargetChanged - the server-pushed retarget (s2c 0x058 ASSIST).
// Nothing in the snapshot carries the server's chosen target, so /assist and
// auto-target-after-kill had no way to move the client's target cursor.
// v24: Entity.monstrosity and CharFlags.{invis, job_master_display}.
// v23: SceneSnapshot.death_menu_offer — the durable s2c 0x0F9 Raise/Reraise or
// Tractor offer shown while dead. (Upstream's "v20"; renumbered on merge because our
// side had already spent 20-22 on zone_generation / untargetable / name_vis.)
// v22: Entity.name_vis is now Option<u8> — None until a General-block update carries
// it. The byte rides UPDATE_HP (entity_update.cpp CEntityUpdatePacket::updateWith), not the Position block,
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
pub const PROTOCOL_VERSION: u32 = 46;

/// Longest countdown `SceneSnapshot::status_icon_expiries` can carry. The
/// producer rejects anything beyond it as a corrupt 0x063 timestamp, and the HUD
/// reserves label width for the widest string inside the same bound, so the two
/// cannot drift into a countdown nothing has room to draw.
pub const MAX_STATUS_TIMER_SECS: u32 = 100 * 3600;

/// vendor/server/data/enums/name_vis.yaml NAMEVIS VIS_HIDE_NAME
pub const NAMEVIS_HIDE_NAME: u8 = 0x08;

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

// vendor/server/data/enums/weather.yaml Weather (None=0..Darkness=19)
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
        // vendor/server/data/enums/weather.yaml Weather
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
        // weather.h Weather notes a repeating 0x14-0x27 set whose usage is unknown;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        door_id: Option<[u8; 4]>,
    },
    Transport {
        size: u16,
        model_id: Option<u32>,
        animation_start: Option<u32>,
        /// Seconds a lift spends between floors, elevators only; `None` for
        /// ships and for a producer older than v41.
        travel_secs: Option<u8>,
    },
}

macro_rules! entity_look_codecs {
    ($($variants:tt)*) => {
        #[derive(Serialize, Deserialize)]
        #[serde(remote = "EntityLook", tag = "kind", rename_all = "snake_case")]
        enum HumanEntityLook { $($variants)* }

        #[derive(Serialize, Deserialize)]
        #[serde(remote = "EntityLook")]
        enum BinaryEntityLook { $($variants)* }
    };
}

entity_look_codecs! {
    Standard { modelid: u16 },
    Equipped {
        face: u8, race: u8, head: u16, body: u16, hands: u16, legs: u16,
        feet: u16, main: u16, sub: u16, ranged: u16,
    },
    Door { size: u16, #[serde(default)] door_id: Option<[u8; 4]> },
    Transport {
        size: u16,
        #[serde(default)] model_id: Option<u32>,
        #[serde(default)] animation_start: Option<u32>,
        #[serde(default)] travel_secs: Option<u8>,
    },
}

impl Serialize for EntityLook {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            HumanEntityLook::serialize(self, serializer)
        } else {
            BinaryEntityLook::serialize(self, serializer)
        }
    }
}

impl<'de> Deserialize<'de> for EntityLook {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if deserializer.is_human_readable() {
            HumanEntityLook::deserialize(deserializer)
        } else {
            BinaryEntityLook::deserialize(deserializer)
        }
    }
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
    pub graph_size: u8,
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

    /// `Flags4.JobMasterFlag` (bit 6 of the u8 at body offset 0x2F): LSB's
    /// job-master display toggle — `SUPERIOR_LEVEL == 5 && m_jobMasterDisplay`
    /// (vendor/server/src/map/packets/char_update.cpp CCharUpdatePacket::updateWith), written on every
    /// non-despawn 0x0D outside all SendFlg blocks. Drives the same nameplate
    /// star as `lfg_master`, which retail keys off `Flags3.LfgMasterFlag` — a
    /// flag LSB hardcodes to 0 (char_update.cpp CCharUpdatePacket::updateWith).
    #[serde(default)]
    pub job_master_display: bool,

    /// `Flags1.InvisFlag` (bit 29): the server's player-invisibility bit — set
    /// for PCs only, when a GM hides themselves or an EFFECTFLAG_INVISIBLE
    /// status effect is active. Retail keeps such players targetable but draws
    /// nothing: no model, no nameplate.
    #[serde(default)]
    pub invis: bool,

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

    /// `GP_SERV_CHAR_PC.MonstrosityFlags` (body 0x3A) — the character is a
    /// monstrosity (Feretory). Written in the Model block only; drives the retail
    /// Monstrosity nameplate marker. See ffxi-proto's `PosHead::monstrosity`.
    /// research/XiPackets/world/server/0x000D/
    #[serde(default)]
    pub monstrosity: bool,

    /// entity_update byte 0x2B (LSB `namevis`; PosHead `flags3 >> 24`), written
    /// under UPDATE_HP — vendor/server/src/map/packets/entity_update.cpp CEntityUpdatePacket::updateWith.
    /// `None` until the first General-block update carries it; treated as visible,
    /// matching the server's VIS_NONE default (base_entity.cpp CBaseEntity::CBaseEntity). LSB NAMEVIS
    /// (vendor/server/data/enums/name_vis.yaml): 0x01 icon, 0x08 hide-name,
    /// 0x80 ghost-phase — the other bits in the data are render-phase flags on real
    /// NPCs (Survival Guides carry 0x20), so only 0x08 suppresses anything.
    #[serde(default)]
    pub name_vis: Option<u8>,
}

// LSB STATUS_TYPE. vendor/server/data/enums/status.yaml.
// Public so the renderer can hide models on INVISIBLE without re-declaring
// the byte (single source of truth).
pub mod status_type {
    pub const NORMAL: u8 = 0;
    pub const UPDATE: u8 = 1;
    pub const DISAPPEAR: u8 = 2;
    pub const INVISIBLE: u8 = 3;
    pub const STATUS_4: u8 = 4;
    pub const CUTSCENE_ONLY: u8 = 6;
    pub const STATUS_18: u8 = 18;
    pub const SHUTDOWN: u8 = 20;
}

// Retail's decode of the wire movement/animation speed bytes (research/XiPackets
// world/server/0x000E): MovementSpeed2 = Speed * 0.1 yps drives how fast a model walks toward
// its target, and AnimationSpeed = SpeedBase * 0.1 scales walk/run clip playback. The two are
// separate values; the run factor multiplies `speed` only (vendor/server/src/map/entities/
// battleentity.cpp CBattleEntity::UpdateSpeed), never `animationSpeed`. These live in
// kuluu-snapshot so both the session reactor and the render layer read one source of truth.
pub mod speed {
    /// Retail's own decode of the wire speed byte (research/XiPackets world/server/0x000E,
    /// MovementSpeed2 = Speed * 0.1 yps).
    pub const SPEED_TO_YPS: f32 = 0.1;

    // The server does not send a faster speed to a mounted player; LSB caps its mount speed at
    // map.MOUNT_SPEED/2 = 40, below the 50 it sends on foot (vendor/server/src/map/entities/
    // battleentity.cpp CBattleEntity::UpdateSpeed). Retail makes up the difference in the client,
    // doubling the decoded speed while mounted and then clamping (research/XIClient/src/XIClient/source/World/
    // Actor/ControllableActor.cpp ControllableActor::StepControl). Taking the packet at face value
    // therefore makes mounting slower.
    pub const MOUNTED_SPEED_MULTIPLIER: f32 = 2.0;

    /// The retail client's movement ceiling in yalms per second (ControllableActor::StepControl).
    pub const MAX_MOVE_SPEED_YPS: f32 = 30.0;

    /// The speed LSB sends an unmounted PC, which every "step per tick" budget in the reactor is
    /// calibrated against (vendor/server/src/map/entities/battle_entity.cpp CBattleEntity::UpdateSpeed).
    pub const BASE_PACKET_SPEED: u8 = 50;

    /// The movement rate a walk/run clip is authored at: the base packet speed decoded to yalms per
    /// second. Retail's AnimationSpeed (SpeedBase * 0.1) scales clip playback relative to this, so a
    /// slower mob walks in slow motion and a faster one in fast forward (research/XiPackets
    /// world/server/0x000E).
    pub const AUTHORED_ANIM_RATE: f32 = BASE_PACKET_SPEED as f32 * SPEED_TO_YPS;

    /// Yalms per second for a decoded packet speed. `speed_base` is a separate value retail keeps but
    /// does not spend on the movement rate; StepControl reads only the doubled-and-clamped `speed`, so
    /// scaling by `speed / speed_base` would under-drive a mounted PC rather than over-drive it.
    pub const fn move_speed_yps(packet_speed: u8, mounted: bool) -> f32 {
        let speed = packet_speed as f32 * SPEED_TO_YPS;
        let speed = if mounted {
            speed * MOUNTED_SPEED_MULTIPLIER
        } else {
            speed
        };
        speed.min(MAX_MOVE_SPEED_YPS)
    }

    /// Movement rate as a multiple of the unmounted run the callers' per-tick step budgets are sized for.
    pub fn move_speed_ratio(packet_speed: u8, mounted: bool) -> f32 {
        move_speed_yps(packet_speed, mounted) / move_speed_yps(BASE_PACKET_SPEED, false)
    }

    // research/XIClient/src/XIClient/source/World/Actor/ControllableActor.cpp ControllableActor::AdjustAnalogKeyLength
    // research/XIClient/src/XIClient/source/World/Actor/BaseActor.cpp BaseActor::GetWalkSpeed
    pub const WALK_SPEED_DIVISOR: f32 = 3.0;

    /// The walk-lock multiplier on a run speed: the analog walk band's length.
    pub const WALK_SPEED_SCALE: f32 = 1.0 / WALK_SPEED_DIVISOR;

    /// BaseActor::GetWalkSpeed: the raw packet speed over three, with neither the mount doubling
    /// nor the clamp `move_speed_yps` applies.
    pub const fn walk_speed_yps(packet_speed: u8) -> f32 {
        packet_speed as f32 * SPEED_TO_YPS / WALK_SPEED_DIVISOR
    }

    /// Walk/run clip playback scale for a decoded animationSpeed byte, relative to the authored
    /// rate. Retail's AnimationSpeed = SpeedBase * 0.1 yps (research/XiPackets world/server/0x000E)
    /// and the clips are authored at AUTHORED_ANIM_RATE, so the ratio is the playback multiplier:
    /// a slower base walks in slow motion, a faster one in fast forward. A zero base (LSB ships
    /// NPCs with speedsub 0) carries no authored rate, so it plays at 1.0 instead of freezing.
    pub const fn anim_rate_scale(speed_base: u8) -> f32 {
        if speed_base == 0 {
            return 1.0;
        }
        (speed_base as f32 * SPEED_TO_YPS) / AUTHORED_ANIM_RATE
    }
}

impl Entity {
    pub fn is_dead(&self) -> bool {
        self.hp_pct == Some(0)
    }

    /// Retail-hidden helper NPC: VIS_HIDE_NAME set — mannequins, "blank"
    /// cutscene actors. vendor/server/src/map/entities/base_entity.cpp CBaseEntity::IsNameHidden
    /// `IsNameHidden() = namevis & FLAG_HIDE_NAME` (0x08); the NAMEVIS enum
    /// defines only 0x01/0x08/0x80, so the other bits are render-phase flags,
    /// not name suppression. Suppresses the nameplate only — never targeting.
    pub fn name_hidden(&self) -> bool {
        self.name_vis.is_some_and(|v| v & NAMEVIS_HIDE_NAME != 0)
    }

    /// LSB STATUS_TYPE::INVISIBLE: the server hides the model entirely —
    /// worms between dive and surface (vendor/server/src/map/ai/controllers/
    /// mob_controller.cpp). The vendored source only sets it on mobs; players
    /// phase via namevis 0x80 instead. Hides model + nameplate; targeting is
    /// already gated by [`Entity::status_selectable`].
    pub fn is_invisible(&self) -> bool {
        self.status == status_type::INVISIBLE
            || (self.status == status_type::DISAPPEAR
                && matches!(self.look, Some(EntityLook::Transport { .. })))
    }

    /// LSB `Flags1.InvisFlag` (bit 29): player-invisibility — a GM hiding
    /// themselves or an EFFECTFLAG_INVISIBLE status effect. The server sets it
    /// for PCs only (vendor/server/src/map/packets/char_update.cpp CCharUpdatePacket::updateWith), so the
    /// kind gate is part of the fact, not a render preference. Unlike
    /// [`Entity::is_invisible`] (STATUS_TYPE on mobs) this never gates targeting:
    /// retail keeps invisible players targetable and draws nothing instead.
    pub fn invis_flag(&self) -> bool {
        matches!(self.kind, EntityKind::Pc) && self.char_flags.invis
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
    /// `look.size == 0x02` identifies a door (LSB `MODEL_DOOR`, which picks the
    /// 0x00E look encoding in vendor/server/src/map/packets/entity_update.cpp).
    /// The only Talk gate LSB puts on it is the Monstrosity guard in
    /// `GP_CLI_COMMAND_ACTION::process`
    /// (`GP_CLI_COMMAND_ACTION_ACTIONID::Talk`, vendor/server/src/map/packets/c2s/0x01a_action.cpp),
    /// which lets a MON use doors and nothing else. The general Talk path
    /// applies no size filter: any NPC within range that `IsSpawned()` with
    /// `status == STATUS_TYPE::NORMAL` (or the player in a Mog House) is
    /// triggered. Limiting `EntityKind::Other` to doors is this client's own
    /// choice; size 3/4 decode to `Transport` (elevators/airships), which stay
    /// non-interactable.
    pub fn is_door(&self) -> bool {
        matches!(self.look, Some(EntityLook::Door { .. }))
    }

    /// The server-side precondition for a door Talk to fire its onTrigger:
    /// LSB's general trigger path (`GP_CLI_COMMAND_ACTION::process`,
    /// vendor/server/src/map/packets/c2s/0x01a_action.cpp) requires
    /// `status == STATUS_TYPE::NORMAL` (vendor/server/data/enums/status.yaml).
    /// Otherwise nothing triggers and the handler falls through to the
    /// `GP_SERV_COMMAND_EVENTUCOFF` release it answers every unlocked Talk with.
    /// Doors do ship with other statuses (DISAPPEAR, STATUS_4, CUTSCENE_ONLY in
    /// vendor/server/sql/npc_list.sql), all already rejected by the blacklist;
    /// the exact match here mirrors the server condition instead of relying on
    /// that blacklist staying complete.
    pub fn is_door_triggerable(&self) -> bool {
        self.is_door() && self.status == status_type::NORMAL
    }

    /// Selectable by click / `<t>`. Dead players stay selectable so a healer can
    /// target them to Raise; dead mobs/NPCs do not. `Other` entities are not
    /// selectable except doors, whose Talk interaction is the retail door flow
    /// and which the server only answers while [`Entity::is_door_triggerable`].
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
        if self.is_door() && !self.is_door_triggerable() {
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
    /// "no party"). vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp GP_SERV_COMMAND_GROUP_LIST::GP_SERV_COMMAND_GROUP_LIST.
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
    /// Attack sent, server not yet answered; see kuluu-session
    /// ReactorGoalSnapshot::Engaging.
    Engaging {
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

    /// An incoming party/alliance invite (s2c 0x0DC) not yet answered with c2s
    /// GROUP_SOLICIT_RES. Retail parks this in the static party state that the
    /// Party row's helper reads to open `prtyjoin` instead of a roster window
    /// (FFXiMain.dll retail-2026-09 RVA 0x2016E0; vault main_menu.md section 24).
    #[serde(default)]
    pub party_invite: Option<PartyInvite>,

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

    /// Targid of the player's own up pet, from 0x068 PetSync (sent to the
    /// owner only); `None` while the pet is down.
    /// vendor/server/src/map/packets/pet_sync.cpp
    #[serde(default)]
    pub self_pet_targid: Option<u16>,

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
    /// (`vendor/server/src/map/packets/char_status.cpp CCharStatusPacket::CCharStatusPacket` — `PChar->animation`).
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
    #[serde(default)]
    pub voyage: Option<Voyage>,

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

    /// The player-trade session from the s2c 0x021/0x022/0x023/0x025 family
    /// (vendor/server/src/map/items/transactions/player_trade.cpp; vault
    /// main_menu.md §25). Trailing on purpose: the postcard pin stays an append.
    #[serde(default)]
    pub trade: Option<Box<TradeWindow>>,

    /// The last completed player-search result set (the `scresult` singleton's
    /// data in retail; vault main_menu.md §26). Kept across frames until a new
    /// query replaces it, exactly like the retail model window holds its rows.
    #[serde(default)]
    pub search: Option<Box<SearchResults>>,

    /// The two link-shell slots as s2c 0x0E0 last reported them (vault main_menu.md §27; the
    /// server pushes both at login and again after every equip/unequip/create). Trailing on
    /// purpose, so the postcard pin stays an append.
    #[serde(default)]
    pub linkshells: LinkShells,

    /// The last s2c 0x06F answer to our synthesis ask (vault main_menu.md §28;
    /// the `cmbhlst` list's feedback row). Kept until the next ask answers, so
    /// a closed craft window still shows what happened. Trailing on purpose.
    #[serde(default)]
    pub synth_result: Option<SynthOutcome>,
    /// The s2c 0x056 mission/quest log battery (vault main_menu.md §30).
    #[serde(default)]
    pub missions: Missions,

    /// The automaton as s2c 0x044 EXTENDED_JOB PUP last reported it — the
    /// pupequip variant's data source for a Puppetmaster (vault main_menu.md
    /// §39; layout vendor/server/src/map/packets/s2c/0x044_extended_job_pup.h).
    /// LSB sends it at login while PUP is main or sub, so its presence mirrors
    /// the DLL's Equipment-row job gate exactly. Trailing on purpose.
    #[serde(default)]
    pub automaton: Option<Box<AutomatonInfo>>,

    /// The monstrosity session as s2c 0x044 EXTENDED_JOB MON last reported it
    /// — the `mp_mncst` ("Customization") variant's data source; `Some` is also
    /// kuluu's stand for the DLL's Monstrosity predicate 0x97D50 on the Equipment
    /// row (vault main_menu.md §39). Trailing on purpose.
    #[serde(default)]
    pub monstrosity: Option<MonstrosityInfo>,

    /// The character's locally-held macros — the mcrmenu family's data model.
    /// Retail keeps them purely client-side (vault main_menu.md §34 wire law:
    /// LSB ships no macro packets [V-negative]); capacity is grounded by the DAT
    /// grids: two palette windows `mcr1pall`/`mcr2pall` whose chunks hold 20
    /// keytop widgets each (vault main_menu.md §41). Only filled slots appear.
    /// Trailing on purpose.
    #[serde(default)]
    pub macros: Vec<MacroSlot>,

    /// The four `SAVE_CONF` bitfield bytes the server last reported for this
    /// character (s2c 0x0B4 GP_SERV_COMMAND_CONFIG; layout vendor/server/src/
    /// common/mmo.h — decode with ffxi_proto::config::parse_config_flags; vault
    /// main_menu.md §42). `None` until the packet arrives (it follows login's
    /// gameok per vendor/server/src/map/packets/c2s/0x00c_gameok.cpp). Trailing
    /// on purpose.
    #[serde(default)]
    pub config_confdata: Option<[u8; 4]>,

    /// The whole job roster as s2c 0x01B JOB_INFO last reported it — the data
    /// source for myroom's `jobchang` (Change Jobs) home-view row, whose pick
    /// list offers every unlocked job and whose commit c2s 0x100 MYROOM_JOB is
    /// validated against exactly this bitmask (vendor/server/src/map/packets/
    /// c2s/0x100_myroom_job.cpp; vault main_menu.md §53). Trailing on purpose.
    #[serde(default)]
    pub jobs: Option<JobList>,

    /// The s2c 0x062 CLISTATUS2 skill block, kept as the raw words so each Status
    /// window can apply its own family's encoding (combat/magic vs craft — see
    /// `ffxi_proto::decode::CharSkillLevels`). `None` until a zone-in carries one.
    #[serde(default)]
    pub char_skills: Option<CharSkills>,

    /// The player's own emote display order (bead kuluu-o9qt7.34): the retail `emote`
    /// window is a **user-arranged list** — its fill reads a per-user blob at open
    /// (FFXiMain.dll retail-2026-09 `.text 0x2259C6` through the user-file helper family),
    /// and `abisortw`'s select drives the swap law in `hud::menu::rearrange_emote_order`.
    /// Entries are emote ids, most-preferred first; unknown ids drop and every id the
    /// store omits keeps its default place behind the arranged ones. Empty = never
    /// arranged (trailing additive field).
    #[serde(default)]
    pub emote_order: Vec<u8>,

    /// The raw body of the last s2c 0x113 `GP_SERV_COMMAND_CURRENCIES_1` report (bead
    /// kuluu-o9qt7.35). Kept RAW on purpose: each list row reads through the client's own
    /// getter table (`kuluu_render::hud::currency_law`, FFXiMain.dll retail-2026-09 fill helper
    /// `.text 0x2297C0`; vault main_menu.md §59 addendum 27), so decoding into typed fields
    /// here would fork that law. `None` until a report lands — and nothing is cached: every
    /// Currencies-window open re-requests (`.text 0xFD520`).
    #[serde(default)]
    pub currencies_1: Option<Vec<u8>>,

    /// The raw body of the last s2c 0x118 `GP_SERV_COMMAND_CURRENCIES_2` report — same law as
    /// [`Self::currencies_1`], answered under c2s 0x115 (`.text 0xFD550`).
    #[serde(default)]
    pub currencies_2: Option<Vec<u8>>,

    /// Which Mog House arrangement mode the client has entered ([`MYROOM_MODE_NORMAL`],
    /// [`MYROOM_MODE_LAYOUT`], ...). Client-side state, never wire data:
    /// retail keeps it in its myroom manager's own word (+0x1C), read back at `.text FFXiMain.dll
    /// retail-2026-09 0x1D34FA`. 0 = normal play, 1 = layout (`/layout`), 2 = gardening (`/garden`),
    /// 3 = flowerpot item pick, 4 = select one piece; anything else is rejected before dispatch.
    #[serde(default)]
    pub myroom_mode: u8,

    /// s2c 0x063 GP_SERV_COMMAND_MISCDATA subtype **Merits** (0x02) — the Status window's
    /// Merit Points row gate and readout source (bead kuluu-o9qt7.43; vault main_menu.md
    /// §59 addendum 36). `None` until that record arrives; retail enables row id 7 exactly when
    /// one has been received AND its `can_use_merit_mode` bit is set (the client stores the pair
    /// at .data FFXiMain.dll retail-2026-09 0x485824 bit1 + 0x485829 & 0x20, gate body
    /// `.text 0x148E88`). Trailing on purpose.
    #[serde(default)]
    pub merits: Option<MisdataMerits>,

    /// The Unity ranking tables (s2c 0x063 type 7) and the s2c 0x110 request mark
    /// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    #[serde(default)]
    pub unity: UnityState,

    /// The last s2c 0x08E alter ego points block; `None` until one arrives this session.
    #[serde(default)]
    pub alter_ego_points: Option<AlterEgoPoints>,

    /// An s2c 0x119 has arrived this session, which readies the Status data the Unity and alter
    /// ego point windows wait for.
    #[serde(default)]
    pub status_data_ready: bool,

    /// s2c 0x00A `LoginState` at the last zone-in (LSB `SAVE_LOGIN_STATE`,
    /// vendor/server/src/map/packets/s2c/0x00a_login.h).
    #[serde(default)]
    pub login_state: u32,

    /// s2c 0x00A `IsMonstrosity` was nonzero at the last zone-in.
    #[serde(default)]
    pub login_is_monstrosity: bool,

    /// The merits s2c 0x08C stored, keyed by merit index (the wire id halved)
    /// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
    #[serde(default)]
    pub merit_entries: std::collections::BTreeMap<u16, MeritEntry>,

    /// Job point type state from s2c 0x08D, keyed by type id (`job * 32 + slot`).
    #[serde(default)]
    pub job_point_types: std::collections::BTreeMap<u16, JobPointType>,

    /// An s2c 0x08D has been processed this session.
    #[serde(default)]
    pub job_points_received: bool,

    /// The last s2c 0x063 JobPoints record; `None` until one arrives.
    #[serde(default)]
    pub job_points: Option<JobPointsRecord>,

    /// The last s2c 0x063 GP_SERV_COMMAND_MISCDATA subtype **Homepoints** (0x06) — the
    /// fast-travel access masks retail copies wholesale into .data FFXiMain.dll retail-2026-09
    /// 0x4858C4 and reads only through per-field bit-test wrappers gating map icons (bead
    /// kuluu-o9qt7.45; vault main_menu.md §59 addendum 37). `None` until that record arrives.
    /// Trailing on purpose.
    #[serde(default)]
    pub homepoints: Option<HomepointMasks>,
}

/// One merit s2c 0x08C stored: the cost of its next upgrade and its current upgrade count
/// (vendor/server/src/map/packets/s2c/0x08c_merit.h `merit_t`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeritEntry {
    pub next: u8,
    pub count: u8,
}

/// One job point type's level and next cost from s2c 0x08D
/// (vendor/server/src/map/packets/s2c/0x08d_job_points.h `jobpoint_t`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPointType {
    pub level: u8,
    pub next: u16,
}

/// [`SceneSnapshot::myroom_mode`] in normal play and in layout mode (`/layout`).
pub const MYROOM_MODE_NORMAL: u8 = 0;
pub const MYROOM_MODE_LAYOUT: u8 = 1;

/// Jobs one s2c 0x063 JobPoints record reports, indexed by job
/// (vendor/server/src/map/packets/s2c/0x063_miscdata_job_points.h `jobs[24]`).
pub const JOB_POINT_JOBS: usize = 24;

/// One job's capacity points, unspent and spent job points.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPointsJob {
    pub capacity_points: u16,
    pub current_jp: u16,
    pub total_jp_spent: u16,
}

/// One s2c 0x063 JobPoints record: the access byte and every job's totals
/// (vendor/server/src/map/packets/s2c/0x063_miscdata_job_points.h).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPointsRecord {
    pub access: u8,
    pub jobs: [JobPointsJob; JOB_POINT_JOBS],
}

/// Mask words per array of [`HomepointMasks`] — LSB sizes each at 4
/// (vendor/server/src/map/packets/s2c/0x063_miscdata_homepoints.h).
pub const HOMEPOINT_MASK_WORDS: usize = 4;

/// One s2c 0x063 Homepoints record: the fast-travel access masks, bit `i & 31` of word `i >> 5`
/// per group (vendor/server/src/map/packets/s2c/0x063_miscdata_homepoints.h; retail's wrapper law
/// `.text FFXiMain.dll retail-2026-09 0x1CBC70`..`0x1CBDB0`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HomepointMasks {
    pub home_point: [u32; HOMEPOINT_MASK_WORDS],
    pub survival_guide: [u32; HOMEPOINT_MASK_WORDS],
    pub waypoint: [u32; HOMEPOINT_MASK_WORDS],
    pub telepoint: u32,
    pub atmos: u32,
    pub eschan_portal: u32,
}

/// Upgrade slots of an alter ego points block, indexed by upgrade id
/// (vendor/server/src/map/packets/s2c/0x08e_alter_ego_points.h `Upgrades[32]` / `Costs[32]`).
pub const ALTER_EGO_UPGRADE_SLOTS: usize = 32;

/// One s2c 0x08E alter ego points block: the balance and, by upgrade id, each upgrade's current
/// level and next cost (vendor/server/src/map/packets/s2c/0x08e_alter_ego_points.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlterEgoPoints {
    pub points: u16,
    pub upgrades: [u8; ALTER_EGO_UPGRADE_SLOTS],
    pub costs: [u16; ALTER_EGO_UPGRADE_SLOTS],
}

/// The Unity Concords one ranking result set reports
/// (vendor/server/src/map/packets/s2c/0x063_miscdata_unity.h `members[11]` / `points[11]`).
pub const UNITY_CONCORDS: usize = 11;

/// The two ranking result sets, LSB `UNITY_RESULTSET` 0 and 1.
pub const UNITY_RESULT_SETS: usize = 2;

/// One Unity ranking result set as LSB lays it out: Concord u (1..=11) at index u - 1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnityRankingSet {
    /// Chunk 1, LSB `members[11]`.
    pub members: [u32; UNITY_CONCORDS],
    /// Chunk 2, LSB `points[11]`.
    pub points: [u32; UNITY_CONCORDS],
    /// The set's last chunk arrived since the last c2s 0x116 for it.
    pub complete: bool,
}

/// Unity ranking data plus the c2s 0x117 request mark
/// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnityState {
    pub rankings: [UnityRankingSet; UNITY_RESULT_SETS],
    /// An s2c 0x110 arrived since the last c2s 0x117.
    pub quest_armed: bool,
}

/// The raw 0x062 block: `CommandRecast[31]` then `skill_base[64]`
/// (vendor/server/src/map/packets/s2c/0x062_clistatus2.h). Retail keeps exactly these
/// words in its character blob and reads them per window
/// (FFXiMain.dll retail-2026-09 skill table RVA 0x4856A4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharSkills {
    pub command_recast: [u32; CharSkills::COMMAND_RECAST_COUNT],
    /// Indexed by skill id: combat 1..=31, magic 32..=47, craft 48..=62. Serde only
    /// codes arrays up to 32 long, so the 64 words travel through [`skill_words`]
    /// as a fixed-length sequence.
    #[serde(with = "skill_words")]
    pub skills: [u16; CharSkills::SKILL_COUNT],
}

/// One s2c 0x063 MISCDATA **Merits** record (bead kuluu-o9qt7.43). LSB defines it as
/// `GP_SERV_COMMAND_MISCDATA::MERITS` (vendor/server/src/map/packets/s2c/0x063_miscdata_merits.h):
/// limitPoints u16, then a bit-word {merit points 7 bits, BLU bonus 6 bits,
/// can-use-merit-mode, xp-capped-or-merit-mode, merit-mode-enabled}, then maxMeritPoints u8.
/// Retail's client copies the two dwords to .data FFXiMain.dll retail-2026-09
/// **0x485826**/**0x48582A** (dispatcher `.text 0x99270`, case subtype 0x02 at `.text 0x99290`)
/// and flags receipt by OR-ing bit 1 into **0x485824**; the Status Merit Points gate
/// (.text `.text 0x148E88`) then tests that flag plus `can_use_merit_mode`
/// (bit 13 of the word = mask 0x20 of byte 0x485829 — vault main_menu.md §59 addendum 36).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MisdataMerits {
    /// Merit (limit) points available to spend.
    pub limit_points: u16,
    /// Unspent merit points, 0..=127 per the 7-bit field.
    pub merit_points: u8,
    /// BLU spell-point bonus, 0..=63 per the 6-bit field.
    pub blu_bonus: u8,
    /// Level >= 75 and holds the Limit Breaker key item — retail's gate for row id 7.
    pub can_use_merit_mode: bool,
    /// XP is capped or merit mode is on.
    pub xp_capped_or_merit_mode: bool,
    /// Merit mode enabled and current job eligible.
    pub merit_mode_enabled: bool,
    /// Maximum merit points for the character.
    pub max_merit_points: u8,
}

/// Fixed-length (de)serialisation for the 64-word `skill_base`: serde's built-in
/// array impls stop at 32, and postcard encodes a fixed tuple with no length byte,
/// so this costs nothing on the wire.
mod skill_words {
    use serde::de::{SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};

    const LEN: usize = crate::CharSkills::SKILL_COUNT;

    pub fn serialize<S: Serializer>(words: &[u16; LEN], s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mut tup = s.serialize_tuple(LEN)?;
        for w in words {
            tup.serialize_element(w)?;
        }
        tup.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u16; LEN], D::Error> {
        struct SkillWordsVisitor;

        impl<'de> Visitor<'de> for SkillWordsVisitor {
            type Value = [u16; LEN];

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{LEN} skill words")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = [0u16; LEN];
                for (i, slot) in out.iter_mut().enumerate() {
                    *slot = seq
                        .next_element::<u16>()?
                        .ok_or_else(|| serde::de::Error::invalid_length(i, &"64 skill words"))?;
                }
                Ok(out)
            }
        }

        d.deserialize_tuple(LEN, SkillWordsVisitor)
    }
}

impl CharSkills {
    pub const COMMAND_RECAST_COUNT: usize = 31;
    pub const SKILL_COUNT: usize = 64;

    /// An all-zero block, i.e. every combat/magic skill at level 0 and no craft
    /// row hidden — only used as a default before a real packet arrives.
    pub fn zeroed() -> Self {
        Self {
            command_recast: [0; Self::COMMAND_RECAST_COUNT],
            skills: [0; Self::SKILL_COUNT],
        }
    }
}

/// The automaton record folded from one s2c 0x044 EXTENDED_JOB PUP frame
/// (vendor/server/src/map/packets/s2c/0x044_extended_job_pup.h; vault
/// main_menu.md §39). `head` is the LSB `AutomatonHead` enum value (0 = none;
/// 1..6 per vendor/server/src/map/enums/automaton.h), `frame` the
/// `AutomatonFrame` value (0x20..0x23), and each `attachments` byte carries one
/// slot's low-byte item id in DAT row order (`AutomatonSlot`: Head=0, Frame=1,
/// Attachment1..12 = 2..13 per vendor/server/src/map/packets/c2s/
/// 0x102_extended_job.h). Unlocked masks follow the packet's uint32 fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomatonInfo {
    pub is_sub_job: bool,
    pub head: u8,
    pub frame: u8,
    pub attachments: [u8; 12],
    pub unlocked_heads: u32,
    pub unlocked_frames: u32,
}

/// The monstrosity record folded from one s2c 0x044 EXTENDED_JOB MON frame
/// (vendor/server/src/map/packets/s2c/0x044_extended_job_mon.h; vault
/// main_menu.md §39): current species id plus the 12 equipped-instinct words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonstrosityInfo {
    pub species: u16,
    pub equipped_instincts: [u16; 12],
}

/// One filled macro slot of the client-local store (vault main_menu.md §41):
/// `block` 0..=1 picks `mcr1pall`/`mcr2pall`, `index` 0..=19 the keytop column
/// in that window's grid, and `text` the stored chat line — a macro is sent as
/// if typed when used (the pager/candidate law of handler FFXiMain.dll
/// retail-2026-09 RVA 0x1A6EB0 pages result candidates through the same bodies).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroSlot {
    pub block: u8,
    pub index: u8,
    pub text: String,
}

/// The job roster record folded from one s2c 0x01B JOB_INFO frame (vendor/
/// server/src/map/packets/s2c/0x01b_job_info.h): `unlocked` is the LSB
/// `jobs.unlocked` bitmask (bit position = JOBTYPE id, bit 0 unused), and
/// `job_levels` is indexed the same way. LSB's MYROOM_JOB validator tests each
/// non-zero index against this mask before changing jobs (vendor/server/src/
/// map/packets/c2s/0x100_myroom_job.cpp — vault main_menu.md §53).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobList {
    pub unlocked: u32,
    pub sub_job_unlocked: bool,
    /// Indexed by JOBTYPE id; entries past the packet's job count stay 0.
    pub job_levels: Vec<u8>,
}

/// One synthesis answer (subset of the s2c 0x06F GP_SERV_COMMAND_COMBINE_ANS
/// body). `result` is a SynthesisResult code
/// (vendor/server/src/map/enums/synthesis_result.h); LSB never fills the
/// packet's `grade`, so kuluu carries it through untouched. `broken_items`
/// lists each ingredient id that broke, at its recipe position (the BreakNo
/// law in vendor/server/src/map/packets/s2c/0x06f_combine_ans.cpp).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthOutcome {
    pub result: u8,
    pub grade: i8,
    /// `Count` — quantity gained (0 unless the synthesis produced an item).
    pub count: u8,
    /// `ItemNo` — the item produced; 0 while nothing came of the ask.
    pub item_no: u16,
    pub broken_items: Vec<u16>,
}

/// One OTHER-port frame of the s2c 0x056 battery: `uint32 Data[8]` verbatim under
/// its Port (port enums in vendor/server/src/map/enums/mission_log.h +
/// quest_log.h; vault main_menu.md §30).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionLogEntry {
    pub port: u16,
    pub data: [u32; 8],
}

/// The s2c 0x056 main-port (0xFFFF) fields verbatim — same law as
/// `ffxi_proto::mission::MissionMain` (vendor/server/src/map/packets/s2c/
/// 0x056_mission.{h,cpp}; vault main_menu.md §30).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionInfo {
    pub nation: u32,
    pub nation_mission: u32,
    pub rotz: u32,
    pub cop: u32,
    /// `statusUpper << 16 | statusLower` as LSB builds it.
    pub cop2: u32,
    pub addon_acp: u8,
    pub addon_amk: u8,
    pub addon_asa: u8,
    pub tales_beginning: u16,
    pub soa: u32,
    pub rov: u32,
}

/// The whole s2c 0x056 battery as folded from login + later updates (vault
/// main_menu.md §30); logs carry every OTHER port last reported.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Missions {
    pub info: Option<MissionInfo>,
    /// Raw `Expansion_TVR` from the 0xFFFE-port frame; 0 while unreported.
    pub tvr: u32,
    pub logs: Vec<MissionLogEntry>,
}

/// One staged slot of a player-trade offer. A gil row keeps `item_no` at the
/// 0xFFFF sentinel (`ffxi_proto::decode::TRADE_GIL_ITEM_NO`) with `qty` as the
/// amount — LSB's own reading (player_trade.cpp canReceive sums sentinel slots
/// as offered gil; an empty view carries the same sentinel with qty 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeOffer {
    pub item_no: u16,
    pub qty: u32,
}

/// The live player-to-player trade. One window per side in retail (`trade` +
/// `gift`, vault main_menu.md §25); kuluu mirrors the pair as one state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeWindow {
    /// The partner's char id (UniqueNo on the wire).
    pub partner_id: u32,
    /// True while we are the invited side and have not answered s2c 0x021 yet;
    /// the window then shows its answer rows instead of slots.
    #[serde(default)]
    pub incoming_pending: bool,
    /// True once the server confirmed Start to both sides — slot staging is
    /// legal only in this phase (retail gates its select rows on it likewise).
    #[serde(default)]
    pub active: bool,
    /// Our offer slots, list position = TradeIndex (slot 0 is gil, per LSB's
    /// `GilSlot = 0`); at most ffxi_proto::decode::TRADE_SLOT_COUNT entries.
    #[serde(default)]
    pub my_slots: Vec<Option<TradeOffer>>,
    /// Partner's offer slots as pushed by s2c 0x023, same index law.
    #[serde(default)]
    pub partner_slots: Vec<Option<TradeOffer>>,
    /// Our Make is sent-and-waiting (retail sets its state to confirmed at send
    /// time, select id 9 → state 4; vault main_menu.md §25).
    #[serde(default)]
    pub my_confirmed: bool,
    /// Partner pressed confirm (s2c RES Make addressed to us names it; any slot
    /// change from either side un-confirms both per player_trade.cpp setSlot).
    #[serde(default)]
    pub partner_confirmed: bool,
}

/// One player row from a `CSearchListPacket` (vendor/server/src/search/packets/
/// search_list.cpp AddPlayer; vault main_menu.md §26). The optionality follows
/// the wire law: fields vanish while flags1 & 0x4000 hides a player's shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPlayer {
    pub name: String,
    /// Zone id where the server last saw them (`Area` entry).
    pub zone: u16,
    #[serde(default)]
    pub nation: Option<u8>,
    #[serde(default)]
    pub main_job: Option<u8>,
    #[serde(default)]
    pub sub_job: Option<u8>,
    #[serde(default)]
    pub main_level: Option<u8>,
    #[serde(default)]
    pub sub_level: Option<u8>,
    #[serde(default)]
    pub race: Option<u8>,
    #[serde(default)]
    pub rank: Option<u8>,
    pub flags1: u16,
    /// 20-bit `Id` entry (the chars.id on the server side).
    pub id: u32,
    /// seacom comment type, present only when non-zero.
    #[serde(default)]
    pub comment_type: Option<u32>,
    pub flags2: u32,
    /// Language bits (`Language` entry).
    pub languages: u16,
}

/// One link-shell slot's equipped pearl (the s2c 0x0E0 model in vault main_menu.md §27).
/// `category`/`item_index` stay 0 while nothing is equipped: LSB leaves both zeroed when
/// `equipLocation(SLOT_LINK1/SLOT_LINK2)` has no entry, and reads them back the same way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkShellSlot {
    /// Container holding the pearl (`Category`).
    pub category: u8,
    /// Slot inside that container (`ItemIndex`); 0 means the shell slot is empty.
    pub item_index: u8,
}

impl LinkShellSlot {
    pub const EMPTY: Self = Self {
        category: 0,
        item_index: 0,
    };

    /// A pearl sits in this slot (the s2c 0x0E0 law above).
    pub fn is_occupied(&self) -> bool {
        self.item_index != 0
    }
}

/// Both link-shell slots — `LinkshellNum` 1 and 2, the only two the server tracks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkShells {
    pub one: LinkShellSlot,
    pub two: LinkShellSlot,
}

impl LinkShells {
    /// Slot by `LinkshellId` (1/2); anything else is out of range and ignored.
    pub fn slot_mut(&mut self, linkshell_id: u8) -> Option<&mut LinkShellSlot> {
        match linkshell_id {
            1 => Some(&mut self.one),
            2 => Some(&mut self.two),
            _ => None,
        }
    }

    pub fn slot(&self, linkshell_id: u8) -> Option<&LinkShellSlot> {
        match linkshell_id {
            1 => Some(&self.one),
            2 => Some(&self.two),
            _ => None,
        }
    }
}

/// Everything one player search returned across its result frames, plus the
/// server-declared total (which may exceed what was sent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub total: u16,
    /// Concatenation of every non-final frame in arrival order, then the final.
    pub players: Vec<SearchPlayer>,
}

impl TradeWindow {
    /// Fresh outgoing window after our TRADE_REQ went out — retail opens its
    /// windows on send (helper 0x13FE70's success path, vault main_menu.md §25).
    pub fn outgoing(partner_id: u32) -> Self {
        Self {
            partner_id,
            incoming_pending: false,
            active: false,
            my_slots: Vec::new(),
            partner_slots: Vec::new(),
            my_confirmed: false,
            partner_confirmed: false,
        }
    }
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

    /// Whether the self player is inside their Mog House. The s2c 0x00A myroom
    /// cluster decides it alone; the party `in_mog_house` flag describes other
    /// members' display state, not ours. Mirrors
    /// `SessionState::self_in_mog_house` on the producer side.
    /// vendor/server/src/map/packets/s2c/0x00a_login.cpp
    pub fn self_in_mog_house(&self) -> bool {
        self.myroom.is_some()
    }
}

/// s2c 0x00A myroom cluster; `model` is an interior model id, not a zone id —
/// resolve via `ffxi_dat::zone_dat::effective_zone_dat_file_id`
/// (vendor/server/src/map/packets/s2c/0x00a_login.cpp GetMogHouseModelID).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyRoom {
    pub model: u16,
    pub sub_map: u8,
    /// Whether the Mog House door is currently open to visitors. Retail keeps it as one client
    /// byte (game-state `+0x40C40`, getter FFXiMain.dll retail-2026-09 `.text 0xF96E0`) whose only
    /// writer is the s2c 0x067 entity update (`.text 0xF9740`, called at `.text 0x9D075`) — and LSB
    /// ships no such packet, so kuluu owns the byte and flips it optimistically together with the
    /// MYROOM_IS Open/Close it sends. Reset law is LSB's note: zoning or moving floors clears the
    /// open flag (vendor/server/src/map/packets/c2s/0x0cb_myroom_is.cpp).
    #[serde(default)]
    pub door_open: bool,
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
    /// EXP between the current and next level, straight from s2c 0x061's
    /// exp_now/exp_next pair (vendor/server/src/map/packets/s2c/0x061_clistatus.h
    /// CLISTATUS); retail draws both in the `level` box under the persona panel.
    #[serde(default)]
    pub exp_now: i16,
    #[serde(default)]
    pub exp_next: i16,
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
    /// `su_lv` from s2c 0x061 — the superior level, shown as `Su` on the persona
    /// panel (FFXiMain.dll retail-2026-09 RVA 0x147287) and used to refuse gear whose
    /// `su` exceeds it.
    #[serde(default)]
    pub su_lv: u8,
    /// bit 0 of s2c 0x061 `mastery_info.flags`: master-level play unlocked. Retail
    /// greys the Status window's Master Levels row without it (and without a level-99
    /// main job) — FFXiMain.dll retail-2026-09 `.text 0x148B20`.
    #[serde(default)]
    pub master_level_unlocked: bool,
    /// s2c 0x061 packet +0x60 — per-slot equip-block mask. The client's equip legality check
    /// refuses when an item's record slot mask intersects it (FFXiMain.dll retail-2026-09
    /// `.text 0xFD350` reads the stored dword; intersection test at `.text 0x132111`).
    #[serde(default)]
    pub equip_slot_block: u32,
    /// `mjob_no` from s2c 0x061 — main job 23 (MON) refuses the alter ego point upgrades
    /// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    #[serde(default)]
    pub mjob_no: u8,
    /// `unity_info.Faction` / `unity_info.Points` and `unity_points1` / `unity_points2` from
    /// s2c 0x061 (vendor/server/src/map/packets/s2c/0x061_clistatus.h CLISTATUS).
    #[serde(default)]
    pub unity_faction: u8,
    #[serde(default)]
    pub unity_points: u32,
    #[serde(default)]
    pub unity_points1: u16,
    #[serde(default)]
    pub unity_points2: u16,
    /// `mjob_lv` from s2c 0x061, the main job level the merit and job point windows gate on
    /// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
    #[serde(default)]
    pub mjob_lv: u8,
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
    /// vendor/server/src/map/packets/c2s/0x106_bazaar_buy.cpp GP_CLI_COMMAND_BAZAAR_BUY::process totalPrice).
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
    /// The server has the slot marked unpickable for as long as an action owns it
    /// (`ffxi_proto::decode::lock_flg::NO_SELECT`). Distinct from `locked`, which every
    /// equipped or reserved item carries all the time.
    #[serde(default)]
    pub unselectable: bool,
    /// Current charges of a charged (usable/enchanted) item; `None` for
    /// non-charged items. From item extdata
    /// (vendor/server/src/map/items/exdata/timer_info.h ItemTimerInfo Header, memcpy'd at
    /// 0x020_item_attr.cpp GP_SERV_COMMAND_ITEM_ATTR::GP_SERV_COMMAND_ITEM_ATTR).
    #[serde(default)]
    pub charges_remaining: Option<u8>,
    /// Absolute Vana'diel next-use timestamp (Earth seconds since the vanadiel
    /// epoch), `None` for non-charged items. Not zeroed on the ready path — LSB
    /// only writes it on cooldown (0x020_item_attr.cpp GP_SERV_COMMAND_ITEM_ATTR::GP_SERV_COMMAND_ITEM_ATTR) — so gate on
    /// `ts > now`, not `ts == 0`.
    #[serde(default)]
    pub next_use_vana_ts: Option<u32>,
    /// Equip-delay end in the same timestamp frame as `next_use_vana_ts`.
    #[serde(default)]
    pub use_delay_end_vana_ts: Option<u32>,
    /// `GP_SERV_COMMAND_ITEM_ATTR` ready flag; `None` for non-charged items.
    #[serde(default)]
    pub ready: Option<bool>,

    /// The record's whole 24-byte Attr, verbatim as s2c 0x020 carried it, or `None` when no Attr
    /// record has been seen for the slot. Kept raw rather than pre-chewed because the bytes answer
    /// several unrelated questions and only the consumer knows which one it is asking: charge/recast
    /// fields above, and the Mog House furniture layer/pose that the two Mog House shelves use
    /// (vendor/server/src/map/items/exdata/furniture.h; FFXiMain.dll retail-2026-09 keeps them on the
    /// record at `.text 0xEC630` and reads pose straight out of them at `.text 0x1CCFD1`).
    #[serde(default)]
    pub extdata: Option<[u8; 24]>,
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
    /// Whether ESC may cancel this event (retail's CliEventCancelFlag; the VM's
    /// 0x42 disarms it in cutscenes that lock you in, 0x2E re-arms). Defaults to
    /// true so frames from an unknown producer stay cancellable.
    /// research/XiEvents/OpCodes/0x0042.md, 0x002E.md
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

    /// How many rows s2c 0x03E SHOP_OPEN said to expect.
    /// vendor/server/src/map/packets/s2c/0x03e_shop_open.cpp
    #[serde(default)]
    pub expected_items: u16,

    /// The final s2c 0x03C page has landed, so `items` is the whole stock.
    /// vendor/server/src/map/packets/s2c/0x03c_shop_list.cpp
    #[serde(default)]
    pub complete: bool,

    /// The vendor NPC's entity id, or 0 when it could not be resolved.
    #[serde(default)]
    pub vendor_id: u32,

    /// A sale the server has priced, awaiting the player's yes/no.
    #[serde(default)]
    pub pending_sale: Option<ShopSale>,
}

/// A pending party/alliance invite, from GroupSolicitReq (s2c 0x0DC,
/// vendor/server/src/map/packets/s2c/0x0dc_group_solicit_req.h).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartyInvite {
    /// Inviter's char id (`UniqueNo`).
    pub char_id: u32,
    /// Inviter's display name (`sName`).
    pub name: String,
    /// True when Kind is PartyKind::Alliance (5); party invites are false.
    #[serde(default)]
    pub alliance: bool,
}

/// A sale appraised by s2c 0x03D and not yet confirmed with c2s 0x085.
/// vendor/server/src/map/packets/s2c/0x03d_shop_sell.cpp, c2s/0x085_shop_sell_set.cpp
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShopSale {
    pub item_index: u8,
    pub item_no: u16,
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
pub struct SceneDelta {
    pub stage: Option<Stage>,
    pub zone_id: Option<u16>,
    pub self_pos: Option<Position>,
    pub entities_upserted: Vec<Entity>,
    pub entities_removed: Vec<u32>,
    pub party_upserted: Vec<PartyMember>,
    /// `Some(Some(_))` installs a pending invite (s2c 0x0DC); `Some(None)`
    /// clears it (answered, withdrawn via 0x0DE, or resolved locally).
    /// Boxed like `TranslatedFrame::Snapshot` already boxes its payload: the
    /// nested Option would otherwise push this delta past clippy's
    /// large_enum_variant budget on every consumer frame.
    #[serde(default)]
    pub set_party_invite: Option<Option<Box<PartyInvite>>>,

    /// Replace-or-clear the whole trade window state (last-writer-wins like
    /// `set_party_invite`): every s2c trade event lands as a rebuilt snapshot of
    /// it, mirroring how LSB's own views re-push each changed slot.
    #[serde(default)]
    pub set_trade: Option<Option<Box<TradeWindow>>>,
    pub chat_appended: Vec<ChatLine>,
    pub diagnostics: Option<Diagnostics>,

    /// `Some` = enter/update the Mog House view; `None` = no change (matching
    /// `zone_id`'s merge convention — a delta cannot clear it, so MH exit must
    /// arrive as a full snapshot, which every producer today sends anyway).
    #[serde(default)]
    pub myroom: Option<MyRoom>,

    /// Replace-or-clear the whole search result set (`set_trade` law): one
    /// completed query lands as one rebuilt frame (vault main_menu.md §26).
    #[serde(default)]
    pub set_search: Option<Option<Box<SearchResults>>>,

    /// Replace both link-shell slots (`set_search` law): each s2c 0x0E0 describes one slot, and the
    /// merge rebuilds the pair (vault main_menu.md §27).
    #[serde(default)]
    pub set_linkshells: Option<LinkShells>,

    /// One synthesis answer replaces the previous one whole (s2c 0x06F; vault
    /// main_menu.md §28).
    #[serde(default)]
    pub set_synth_result: Option<SynthOutcome>,
    /// Last-writer-wins the whole battery fold (vault main_menu.md §30).
    #[serde(default)]
    pub set_missions: Option<Missions>,

    /// One s2c 0x044 PUP frame replaces the automaton record whole (vault
    /// main_menu.md §39).
    #[serde(default)]
    pub set_automaton: Option<Option<Box<AutomatonInfo>>>,

    /// One s2c 0x044 MON frame replaces the monstrosity record whole (vault
    /// main_menu.md §39).
    #[serde(default)]
    pub set_monstrosity: Option<Option<MonstrosityInfo>>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutsceneActor {
    LocalPlayer,
    Entity { server_id: u32 },
}

/// The motion resource a LOADEXTSCHEDULER cue loads before playing its key
/// (research/XiEvents/OpCodes/0x005B.md, 0x0066.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtSchedulerMotion {
    /// 0x5B: the event motion resource a single DAT file id names.
    /// research/XiEvents/OpCodes/0x005B.md
    Event(u32),
    /// 0x66 in range: container A (resource tag 1) and the two B candidates
    /// (resource tag 2); the renderer picks between them from the actor's CIB
    /// waist byte.
    /// research/XiEvents/OpCodes/0x0066.md
    Tpc { a: u32, b_set: u32, b_clear: u32 },
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
    /// 0x6C TRANSPAR: fade `target`'s alpha to `end_alpha` (0..=255) over
    /// `duration_frames` frames (research/XiEvents/OpCodes/0x006C.md).
    Transpar {
        target: CutsceneActor,
        end_alpha: i32,
        duration_frames: i32,
    },
    /// Take camera control away from the player, or give it back.
    CameraLock { lock: bool },
    /// The decoded 0x38 mode word; visibility policy is unresolved.
    LocalMode { mode: u16 },
    /// 0x20: write retail's `CliEventUcFlag`; while it holds, the player's
    /// `CanIMove` is false (research/XiEvents/OpCodes/0x0020.md,
    /// research/XIClient ActorTelemetry::CanIMove).
    PlayerControl { locked: bool },
    /// 0x67/0x68 HIDE_HUD/SHOW_HUD: hide or show the entire HUD UI for the
    /// rest of the cutscene (research/XiEvents/OpCodes/0x0067.md, 0x0068.md).
    HudHide { hide: bool },
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
    /// Put the target on or off a mount. `status_event` is the `GameStatus`
    /// value the script writes; `mount_id` is carried only by the non-chocobo
    /// mount cases.
    Mount {
        target: CutsceneActor,
        status_event: u8,
        mount_id: Option<u16>,
    },
    /// Load the motion resource into `actor`, then play action `key` on it
    /// with `partner`. `motion` is `None` for the 0x66 out-of-range package,
    /// where retail logs and loads nothing and the renderer plays `key` on the
    /// actor's own resources.
    /// research/XiEvents/OpCodes/0x0066.md
    ExtScheduler {
        motion: Option<ExtSchedulerMotion>,
        actor: CutsceneActor,
        partner: CutsceneActor,
        key: FourCc,
    },
    /// Start zone-level scheduler routine `key` over the two actors (the
    /// 0x2D/0x54 pair, research/XiEvents/OpCodes/0x002D.md); the host resolves
    /// `key` out of the current zone's own model DAT (`zone_id`, with the
    /// entrance/instance partner and non-model carriers as fallbacks) and its
    /// camera routes drive the operator camera.
    ZoneScheduler {
        key: FourCc,
        actor: CutsceneActor,
        partner: CutsceneActor,
        zone_id: u16,
    },
    /// Walk `actor` to `(x, y, z)` at `speed`, facing `heading`. The
    /// coordinates are the VM's event-coordinate integers; the renderer scales
    /// them with EVENT_COORD_UNITS / EVENT_HEADING_UNITS.
    ActorMove {
        actor: CutsceneActor,
        x: i32,
        y: i32,
        z: i32,
        heading: i32,
        /// Raw 0x32 MainSpeed operand; the renderer scales it with
        /// `ffxi_event::vm::scene::EVENT_SPEED_SCALE`.
        /// research/XiEvents/OpCodes/0x0032.md
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
    ActorFace { actor: CutsceneActor, heading: i32 },
    /// Turn `actor` to face `target`.
    ActorLookAt {
        actor: CutsceneActor,
        target: CutsceneActor,
    },
    /// Stop the current action; `key` supplies the replacement idle motion.
    ActorStopAction {
        actor: CutsceneActor,
        key: Option<FourCc>,
    },
    /// 0xB5 case 0: set `actor`'s display name to `name` (the event's work
    /// string, filled from an inline literal or the s2c 0x005D PENDINGSTR
    /// table); released like every other cue at [`ViewerEvent::CutsceneEnded`].
    /// research/XiEvents/OpCodes/0x00B5.md,
    /// vendor/server/src/map/packets/s2c/0x05d_pendingstr.cpp
    EntityName {
        actor: CutsceneActor,
        name: [u8; 16],
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

    /// s2c 0x058 ASSIST: the server moved our target. `None` is the wire's
    /// zero `AssistNo` (the target went away), which leaves the local target
    /// alone rather than clearing it - retail's `RecvAssist` behaviour for a
    /// zero id is not established.
    /// vendor/server/src/map/packets/s2c/0x058_assist.cpp
    TargetChanged {
        target_id: Option<u32>,
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

    /// Event script 0xC8 MAP_TUTORIAL: open the Map screen on zone `map_id`.
    /// research/XiEvents/OpCodes/0x00C8.md
    MapOpen {
        map_id: u16,
        tutorial: bool,
    },

    /// Event script 0x8B MAP_MARKER: place a named marker at milli-unit
    /// coordinates on zone `map_id`'s map.
    /// research/XiEvents/OpCodes/0x008B.md
    MapMarkerPlaced {
        map_id: u16,
        x_milli: i32,
        y_milli: i32,
        label: String,
    },

    /// Event script 0x8A CLOSE_MAP: close the Map screen.
    /// research/XiEvents/OpCodes/0x008A.md
    MapClosed,

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
        /// First result's raw `animation` index, for every category: the file-table key of
        /// the caster's effect DAT. Absent on a result-less or truncated body.
        animation: Option<u16>,
        /// First result's `(info, hit_distortion, knockback)` bits
        /// (vendor/server/src/map/packets/s2c/0x028_battle2.cpp GP_SERV_COMMAND_BATTLE2::pack),
        /// read for every category; absent when the body carries no result block. `info` is
        /// only meaningful for a basic attack (vendor/server/src/map/enums/action/info.h).
        outcome: Option<(u8, u8, u8)>,
    },

    /// Every target of one 0x028 whose result landed with a knockback level:
    /// `(target_id, level)`, level 1..=7 (vendor/server/src/map/enums/action/knockback.h).
    /// The server moves nobody; the victim's client shoves itself when the
    /// caster's skill routine reaches its knockback stage.
    Knockbacks {
        actor_id: u32,
        hits: Vec<(u32, u8)>,
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

    /// Capture the native client's primary window to PNG via Bevy render-target
    /// readback — no focus or screen-recording permission needed. GUI-side only:
    /// the relay routes it into `DebugControl`, not the session (which treats
    /// `AgentCommand::Screenshot` as a no-op). `None` leaves default naming
    /// (`screenshot-N.png`) to the GUI side.
    Screenshot {
        path: Option<String>,
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

    #[test]
    fn legacy_clock_hold_cue_uses_hour_only_defaults() {
        let cue: super::CutsceneCue = serde_json::from_value(serde_json::json!({
            "ClockHold": {"stop": true, "hour": null}
        }))
        .unwrap();
        assert!(matches!(
            cue,
            super::CutsceneCue::ClockHold {
                stop: true,
                hour: None,
                minute: 0,
                day_from_epoch: None
            }
        ));
    }
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
            // Never arranged: the sample keeps the emote display order (bead
            // kuluu-o9qt7.34) in its untouched, default-list state.
            emote_order: Vec::new(),
            currencies_1: None,
            currencies_2: None,
            // No 0x062 block in the sample: the Status skill rows read as
            // "nothing received yet", exactly like a fresh zone-in.
            char_skills: None,
            merits: None,
            unity: UnityState::default(),
            alter_ego_points: None,
            status_data_ready: false,
            login_state: 0,
            login_is_monstrosity: false,
            merit_entries: Default::default(),
            job_point_types: Default::default(),
            job_points_received: false,
            job_points: None,
            // A received Homepoints record, so the sample exercises the mask words in serde and
            // postcard alike (bead kuluu-o9qt7.45).
            homepoints: Some(HomepointMasks {
                home_point: [0x1, 0x2, 0x3, 0x4],
                survival_guide: [0x10, 0x20, 0x40, 0x80],
                waypoint: [0xFFFF_FFFF, 0, 0, 0],
                telepoint: 0xDEAD_BEEF,
                atmos: 0xCAFE_BABE,
                eschan_portal: 0xF00D_D00D,
            }),
            party_invite: None,
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
                monstrosity: false,
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
            self_pet_targid: Some(0x010E),
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
                door_open: false,
            }),
            // Nothing being arranged in the sample (bead kuluu-o9qt7.39).
            myroom_mode: 0,
            mh_2f_unlocked: None,
            sub_area: None,
            voyage: None,
            emote_jobs: None,
            emote_chairs: None,
            check: None,
            check_message: None,
            widescan: WidescanList::default(),
            death_menu_offer: None,
            trade: None,
            search: None,
            linkshells: LinkShells::default(),
            synth_result: None,
            missions: Missions::default(),
            automaton: None,
            monstrosity: None,
            macros: Vec::new(),
            config_confdata: None,
            jobs: None,
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
        assert!(
            !snap.self_in_mog_house(),
            "the self party member's moghouse flag must not decide where we are"
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
        assert_eq!(door.status, status_type::NORMAL);
        assert!(door.is_door_triggerable());

        let door_updating = Entity {
            status: status_type::UPDATE,
            ..door.clone()
        };
        assert!(
            !door_updating.is_door_triggerable() && !door_updating.is_targetable(),
            "LSB drops the Talk on a door whose status is not NORMAL"
        );

        let npc_updating = Entity {
            kind: EntityKind::Npc,
            status: status_type::UPDATE,
            ..base.clone()
        };
        assert!(
            npc_updating.is_targetable(),
            "the exact-NORMAL gate is door-only; NPCs keep the fail-open blacklist"
        );

        let transport = Entity {
            kind: EntityKind::Other,
            look: Some(EntityLook::Transport {
                size: 3,
                model_id: None,
                animation_start: None,
                travel_secs: None,
            }),
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
    fn entity_look_codecs_preserve_every_variant() {
        const TRANSPORT_POSTCARD: &[u8] = &[3, 3, 1, 14, 1, 192, 196, 7, 1, 8];
        let variants = [
            (EntityLook::Standard { modelid: 321 }, "standard"),
            (
                EntityLook::Equipped {
                    face: 1,
                    race: 2,
                    head: 3,
                    body: 4,
                    hands: 5,
                    legs: 6,
                    feet: 7,
                    main: 8,
                    sub: 9,
                    ranged: 10,
                },
                "equipped",
            ),
            (
                EntityLook::Door {
                    size: 2,
                    door_id: Some(*b"_6ww"),
                },
                "door",
            ),
            (
                EntityLook::Transport {
                    size: 3,
                    model_id: Some(14),
                    animation_start: Some(123_456),
                    travel_secs: Some(8),
                },
                "transport",
            ),
        ];
        for (look, tag) in variants {
            let json = serde_json::to_value(look).unwrap();
            assert_eq!(json["kind"], tag);
            assert_eq!(serde_json::from_value::<EntityLook>(json).unwrap(), look);
            let bytes = postcard::to_allocvec(&look).unwrap();
            assert_eq!(postcard::from_bytes::<EntityLook>(&bytes).unwrap(), look);
            if matches!(look, EntityLook::Transport { .. }) {
                assert_eq!(bytes, TRANSPORT_POSTCARD);
            }
        }
    }

    #[test]
    fn current_protocol_preserves_transport_and_voyage_fields() {
        const VERSION: u32 = 46;
        const STAMP: u32 = 0x1200_3400;
        assert_eq!(PROTOCOL_VERSION, VERSION);
        let mut snapshot = sample_snapshot();
        snapshot.voyage = Some(Voyage {
            start: STAMP,
            duration: 897,
            reverse: true,
            route: 2,
        });
        snapshot.entities[0].look = Some(EntityLook::Transport {
            size: 4,
            model_id: Some(14),
            animation_start: Some(STAMP),
            travel_secs: None,
        });
        let bytes = postcard::to_allocvec(&snapshot).unwrap();
        let decoded: SceneSnapshot = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.voyage, snapshot.voyage);
        assert_eq!(decoded.entities[0].look, snapshot.entities[0].look);
        let mut legacy = serde_json::to_value(&snapshot).unwrap();
        legacy.as_object_mut().unwrap().remove("voyage");
        assert_eq!(
            serde_json::from_value::<SceneSnapshot>(legacy)
                .unwrap()
                .voyage,
            None
        );
        let old_look: EntityLook =
            serde_json::from_value(serde_json::json!({"kind": "transport", "size": 4})).unwrap();
        assert_eq!(
            old_look,
            EntityLook::Transport {
                size: 4,
                model_id: None,
                animation_start: None,
                travel_secs: None,
            }
        );
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
                        sub_map: 0,
                        door_open: false
                    })
                );
                assert_eq!(s.auction, sample_auction());
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn snapshot_extensions_survive_the_postcard_relay() {
        let mut snapshot = sample_snapshot();
        snapshot.zone_generation = 128;
        snapshot.entities[0].char_flags.untargetable = true;
        snapshot.entities[0].char_flags.graph_size = 3;
        snapshot.entities[0].name_vis = Some(0x08);
        snapshot.death_menu_offer = Some(DeathMenuOffer::Tractor);
        let bytes = postcard::to_allocvec(&Frame::Snapshot(Box::new(snapshot))).unwrap();
        let Frame::Snapshot(decoded) = postcard::from_bytes(&bytes).unwrap() else {
            panic!("expected snapshot");
        };
        assert_eq!(decoded.zone_generation, 128);
        assert!(decoded.entities[0].char_flags.untargetable);
        assert_eq!(decoded.entities[0].char_flags.graph_size, 3);
        assert_eq!(decoded.entities[0].name_vis, Some(0x08));
        assert_eq!(decoded.chat[0].text, "hi");
        assert_eq!(decoded.death_menu_offer, Some(DeathMenuOffer::Tractor));
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

    /// The Weather enum's unknown 20-39 ids must not wrap onto real weathers.
    /// vendor/server/data/enums/weather.yaml
    #[test]
    fn from_lsb_unknown_ids_are_none() {
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
            CutsceneCue::PlayerControl { locked: true },
            CutsceneCue::ActorHide {
                target: CutsceneActor::Entity {
                    server_id: 0x010E_6032,
                },
                hide: true,
            },
            CutsceneCue::Transpar {
                target: CutsceneActor::Entity {
                    server_id: 0x010E_6032,
                },
                end_alpha: 0,
                duration_frames: 60,
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
            CutsceneCue::EntityName {
                actor: CutsceneActor::Entity {
                    server_id: 0x010E_602F,
                },
                name: *b"Sajj'aka\0\0\0\0\0\0\0\0",
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
            "self_pet_targid",
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
            "voyage",
            "emote_jobs",
            "emote_chairs",
            "check",
            "check_message",
            "widescan",
            "death_menu_offer",
            "party_invite",
            "trade",
            "search",
            "linkshells",
            "synth_result",
            "missions",
            "automaton",
            "monstrosity",
            "macros",
            "config_confdata",
            "jobs",
            "char_skills",
            "merits",
            "emote_order",
            "currencies_1",
            "currencies_2",
            "myroom_mode",
            "unity",
            "alter_ego_points",
            "status_data_ready",
            "login_state",
            "login_is_monstrosity",
            "merit_entries",
            "job_point_types",
            "job_points_received",
            "job_points",
            "homepoints",
        ];
        want.sort();
        assert_eq!(got, want, "SceneSnapshot fields changed: additive-only, update this pin deliberately and rebuild relay consumers together");
    }

    // Re-pinned for each trailing additive field since §25-era party_invite/trade
    // (then search, then linkshells, then missions (Option tag + u32 + Vec length)): every append is exactly
    // the bytes its default encodes to; no earlier field moved. §59 addendum 31 appends
    // myroom_mode (u8) as exactly one more zero byte at the tail. §59 addendum 36 appends
    // merits (`Option<MisdataMerits>`) as exactly one more `None` tag byte. The Unity / alter ego
    // point fields append 51 zero bytes: `unity` (two sets of 11 + 11 varint zeros and a flag, then
    // the quest flag), the `alter_ego_points` `None` tag, and three scalar zeros. The merit and job
    // point fields append four zero bytes: two empty map lengths, a flag and a `None` tag. §59
    // addendum 37 appends homepoints (`Option<HomepointMasks>`) as exactly one more `None` tag byte.
    const SNAPSHOT_DEFAULT_POSTCARD_HEX: &str = "0000000000000000000000000000000019190000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

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

    /// The authored base plays at unity; halves of it walk in half-speed slow
    /// motion, and a faster base scales up. No kind or threshold anywhere: the
    /// byte is the whole input.
    #[test]
    fn anim_rate_scales_with_speed_base() {
        assert!(
            (speed::anim_rate_scale(50) - 1.0).abs() < 1e-6,
            "authored base plays at unity"
        );
        assert!((speed::anim_rate_scale(25) - 0.5).abs() < 1e-6);
        assert!(speed::anim_rate_scale(75) > speed::anim_rate_scale(50));
        assert!(
            speed::anim_rate_scale(1) > 0.0 && speed::anim_rate_scale(0) == 1.0,
            "a nonzero base keeps a positive scale"
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voyage {
    pub start: u32,
    pub duration: u16,
    pub reverse: bool,
    pub route: u8,
}
