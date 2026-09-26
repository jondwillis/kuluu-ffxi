//! Choreography cues the event VM hands to its host.
//!
//! The dialog opcodes yield through [`crate::StepResult`]; the *staging* opcodes
//! (actor motion, screen fade, camera lock, music volume, event-hide, mount) do
//! not yield at all in retail — they call straight into the renderer and fall
//! through to the next instruction. A cue is that call, captured as data so a
//! host outside this crate can perform it, drained with
//! [`crate::EventVm::take_cues`].
//!
//! Cues are **event-scoped**: each one describes a change retail applies for the
//! duration of the running event, never a persisted flag.

use crate::vm::scene::EventPosition;

/// A baked `XiEvent::GetActorIndex` operand: the entity an opcode names
/// (research/XiEvents/Event VM Functions.md). Cues carry it unresolved because
/// only the host owns the entity table the reserved selectors index; this VM's
/// [`crate::EventVm`] would collapse "the local player" and "the event entity"
/// onto one target index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActorLookup(pub u32);

/// Reserved lookup selectors: `…C0` local player, `…C1`–`…D1` party/alliance
/// slots, `…F1`–`…F5` party members, `…F8` the event entity, `…F9`/`…F0` the
/// local player again (research/XiEvents/Event VM Functions.md).
const LOOKUP_RESERVED: std::ops::RangeInclusive<u32> =
    LOOKUP_LOCAL_PLAYER_A..=LOOKUP_LOCAL_PLAYER_C;
const LOOKUP_LOCAL_PLAYER_A: u32 = 0x7FFF_FFC0;
/// The same marker the zone event DATs use for their master block, so the
/// reserved-range member and the zone-block owner stay one value.
const LOOKUP_LOCAL_PLAYER_B: u32 = ffxi_dat::event_dat::ZONE_PLAYER_ACTOR;
const LOOKUP_LOCAL_PLAYER_C: u32 = 0x7FFF_FFF9;
const LOOKUP_EVENT_ENTITY: u32 = 0x7FFF_FFF8;
/// A lookup with any high byte set is a literal entity server id, whose low bits
/// are the target index (same doc, default handler).
const LOOKUP_SERVER_ID_MASK: u32 = 0xFF << 24;
pub(crate) const LOOKUP_TARGET_INDEX_MASK: u32 = 0x3FF;

impl ActorLookup {
    pub const LOCAL_PLAYER: Self = Self(LOOKUP_LOCAL_PLAYER_B);
    pub const EVENT_ENTITY: Self = Self(LOOKUP_EVENT_ENTITY);
    /// Hold-table stand-in for a zone-level routine (0x2D/0x54): retail polls the
    /// zone object, not an actor, so kuluu keys that WAIT* hold on this sentinel
    /// instead of either cue actor. It sits just past retail's reserved lookup
    /// range (research/XiEvents/Event VM Functions.md GetActorIndex).
    pub const ZONE: Self = Self(0x7FFF_FFFA);

    pub fn is_local_player(self) -> bool {
        matches!(
            self.0,
            LOOKUP_LOCAL_PLAYER_A | LOOKUP_LOCAL_PLAYER_B | LOOKUP_LOCAL_PLAYER_C
        )
    }

    /// True for the explicit event-entity selector and for the default
    /// handler's fallback (a non-reserved value with no high byte set).
    pub fn is_event_entity(self) -> bool {
        self.0 == LOOKUP_EVENT_ENTITY
            || (!LOOKUP_RESERVED.contains(&self.0) && self.0 & LOOKUP_SERVER_ID_MASK == 0)
    }

    /// The literal entity server id this lookup names, if it is one.
    pub fn server_id(self) -> Option<u32> {
        (!LOOKUP_RESERVED.contains(&self.0) && self.0 & LOOKUP_SERVER_ID_MASK != 0)
            .then_some(self.0)
    }

    /// Target index of the literal server id — its low 10 bits.
    pub fn target_index(self) -> Option<u16> {
        self.server_id()
            .map(|id| (id & LOOKUP_TARGET_INDEX_MASK) as u16)
    }
}

/// A four-character scheduler/action key in file byte order — the operand is an
/// ASCII tag (`"fdo0"`, `"kue0"`), not a numeric id.
pub type FourCc = [u8; 4];

/// Base DAT file id opcode 0x45 adds its [`dat_id_helper`]-mapped work operand
/// to (research/XiEvents/OpCodes/0x0045.md, `FUNC_XiEvent_OpCode_0x0045` passing
/// 30704 to `CodeLOADEVENTSCHEDULER2`).
pub const SCHEDULER_DAT_ID_BASE: u32 = 30704;

/// The DAT file id base each 0x45 twin adds its work operand to. Each twin
/// calls `FUNC_XiEvent_CodeLOADEVENTSCHEDULER2` with a fixed second argument
/// and skips the 0x45-only `dat_id_helper` remap, so its DAT id is this base
/// plus the raw work value (research/XiEvents/OpCodes/0x0062.md, 0x009F.md,
/// 0x00BB.md, 0x00C5.md, 0x00CD.md, 0x00D0.md, 0x00D5.md).
pub const fn scheduler_twin_base(op: u8) -> Option<u32> {
    Some(match op {
        0x62 => 5012,
        0x9F => 51183,
        0xBB => 56685,
        0xC5 => 67355,
        0xCD => 70435,
        0xD0 => 70691,
        0xD5 => 102449,
        _ => return None,
    })
}

/// Base DAT file id opcode 0x7D adds its work operand to: the scheduler that
/// runs on the local player (the rank-up animations), its `main` routine on
/// the player with the player as its own target, no `dat_id_helper` remap
/// (research/XiEvents/OpCodes/0x007D.md, `FUNC_LoadStartScheduler(val + 5112, …)`).
pub const LOCAL_PLAYER_SCHEDULER_DAT_ID_BASE: u32 = 5112;

/// Base DAT file id opcode 0x73 MAGICSCHEDULOR adds its work operand to. The
/// operand is a spell animation index: the same column vendor/server
/// sql/spell_list.sql `animation` fills for a cast (Invisible 498, Sneak 499,
/// Deodorize 500 sit beside the gate guard's Signet 497 and home point 504),
/// and the same base a 0x028 magic finish resolves through
/// (ffxi_vocab::action_anim::spell_file_id).
pub const MAGIC_DAT_ID_BASE: u32 = ffxi_vocab::action_anim::SPELL_FILE_TABLE_OFFSET;

/// The routine 0x73 plays out of that DAT: research/XiEvents/OpCodes/0x0073.md
/// passes `0x6E69616D` ("main") to `FUNC_XiActor_Unknown` for every case.
pub const MAGIC_ROUTINE_TAG: FourCc = *b"main";

/// Scheduler DAT holding the screen-fade pair (ROM/62/110.DAT).
pub const SCHEDULER_FADE_DAT_ID: u32 = 30904;

/// Screen fade-out scheduler tag, emitted by [`EventCue::Scheduler`].
pub const SCHEDULER_TAG_FADE_OUT: FourCc = *b"fdo0";
/// Screen fade-in scheduler tag, emitted by [`EventCue::Scheduler`].
pub const SCHEDULER_TAG_FADE_IN: FourCc = *b"fdi0";

/// [`EventCue::Scheduler::duration`] value meaning "play the DAT-authored
/// timing verbatim" — the overwhelming majority of authored call sites.
pub const SCHEDULER_DURATION_FROM_DAT: u16 = 0;

/// The hold key 0x6E/0x63 arm and 0x99 polls: retail's `AnimationPlay` is one
/// per-entity slot, so the wait carries no key operand of its own and keys on
/// this constant (research/XiEvents/OpCodes/0x006E.md, 0x0099.md).
pub const EMOTE_ANIMATION_KEY: FourCc = *b"emot";

/// `GameStatus` values opcode 0x7E writes to the target's `StatusEvent`
/// (research/XIClient/src/XIClient/include/World/Actor/GameStatus.h; the case-to-value mapping is
/// research/XiEvents/OpCodes/0x007E.md).
pub const STATUS_EVENT_IDLE: u8 = 0;
pub const STATUS_EVENT_CHOCOBO: u8 = 5;
pub const STATUS_EVENT_MOUNT: u8 = 85;
/// The door bytes opcodes 0x4C/0x4D write: `GameStatus` `D_OPEN`/`D_CLOSE`
/// (research/XiEvents/OpCodes/0x004C.md, 0x004D.md; research/XIClient/src/XIClient/include/World/Actor/GameStatus.h).
pub const STATUS_EVENT_DOOR_OPEN: u8 = 8;
pub const STATUS_EVENT_DOOR_CLOSE: u8 = 9;
/// 0x4F adds this to its work operand: the `M1`..`M8` event-motion statuses
/// (research/XiEvents/OpCodes/0x004F.md; research/XIClient/src/XIClient/include/World/Actor/GameStatus.h).
pub const STATUS_EVENT_MOTION_BASE: u32 = 18;
/// The second door status pair opcodes 0x8E/0x8F write: `GameStatus`
/// `D_OPEN2`/`D_CLOSE2` (research/XiEvents/OpCodes/0x008E.md, 0x008F.md;
/// research/XIClient/src/XIClient/include/World/Actor/GameStatus.h).
pub const STATUS_EVENT_DOOR_OPEN2: u8 = 45;
pub const STATUS_EVENT_DOOR_CLOSE2: u8 = 46;

/// Highest music-volume table index (`FUNC_YmMusicServer_Volume`'s first
/// argument indexes a volume table; it is not a percentage).
pub const MUSIC_VOLUME_MAX: u8 = 127;

/// The retail sound-type bits the 0x69/0x6A volume opcodes write
/// (research/XiEvents/OpCodes/0x0069.md): which of the client's volume
/// channels the opcode sets.
pub const SOUND_TYPE_EFFECT: u8 = 0x01;
pub const SOUND_TYPE_SYSTEM: u8 = 0x02;
pub const SOUND_TYPE_ZONE: u8 = 0x04;
pub const SOUND_TYPE_MASTER: u8 = 0x08;
pub const SOUND_TYPE_SPECIAL_CHAT: u8 = 0x10;

/// `FUNC_DatIdHelper` (research/XiEvents/OpCodes/0x0045.md): the two folded
/// bands of the scheduler DAT id space.
pub fn dat_id_helper(param: i32) -> i32 {
    const HIGH_BAND: i32 = 600;
    const HIGH_BAND_OFFSET: i32 = 39643;
    const MID_BAND: i32 = 300;
    const MID_BAND_OFFSET: i32 = 25937;
    if param >= HIGH_BAND {
        return param.wrapping_add(HIGH_BAND_OFFSET);
    }
    if param >= MID_BAND {
        return param.wrapping_add(MID_BAND_OFFSET);
    }
    param
}

// The 0x5B event motion resource bands (research/XiEvents/OpCodes/0x005B.md,
// FUNC_XiSkeletonActor_ReadEventMotionRes call sites): the operand selects a
// base DAT id by which band it falls in.
const EVENT_MOTION_BAND_1: i32 = 512;
const EVENT_MOTION_BAND_2: i32 = 1024;
const EVENT_MOTION_BAND_3: i32 = 2048;
pub(crate) const EVENT_MOTION_BAND_4: i32 = 3072;
const EVENT_MOTION_BASE_0: i32 = 32104;
const EVENT_MOTION_BASE_1: i32 = 49135;
const EVENT_MOTION_BASE_2: i32 = 56345;
const EVENT_MOTION_BASE_3: i32 = 59739;
const EVENT_MOTION_BASE_4: i32 = 66339;

/// DAT id of the event motion resource a LOADEXTSCHEDULER operand names.
pub fn event_motion_dat_id(param: i32) -> u32 {
    let base = if param < EVENT_MOTION_BAND_1 {
        EVENT_MOTION_BASE_0
    } else if param < EVENT_MOTION_BAND_2 {
        EVENT_MOTION_BASE_1
    } else if param < EVENT_MOTION_BAND_3 {
        EVENT_MOTION_BASE_2
    } else if param < EVENT_MOTION_BAND_4 {
        EVENT_MOTION_BASE_3
    } else {
        EVENT_MOTION_BASE_4
    };
    param.wrapping_add(base) as u32
}

// The 0x66 Tpc motion package bands (FFXiMain.dll ReadTpcEventMotionRes @rva 0xD2230):
// the package number picks a base by which band it falls in, and each band
// yields three ids - A (resource tag 1) and the two B candidates (resource
// tag 2), one per CIB waist-byte state.
pub const TPC_PACKAGE_OUT_OF_RANGE: u32 = 0x118;
const TPC_PACKAGE_BAND_2: u32 = 0x46;
const TPC_PACKAGE_BAND_3: u32 = 0x8C;
const TPC_PACKAGE_BAND_4: u32 = 0xD2;
const TPC_PACKAGE_A_BASE_1: u32 = 0x7FC8;
const TPC_PACKAGE_B_SET_BASE_1: u32 = 0x800E;
const TPC_PACKAGE_B_CLEAR_BASE_1: u32 = 0x8054;
const TPC_PACKAGE_A_BASE_2: u32 = 0xEF39;
const TPC_PACKAGE_B_SET_BASE_2: u32 = 0xEF7F;
const TPC_PACKAGE_B_CLEAR_BASE_2: u32 = 0xEFC5;
const TPC_PACKAGE_A_BASE_3: u32 = 0x15711;
const TPC_PACKAGE_B_SET_BASE_3: u32 = 0x15757;
const TPC_PACKAGE_B_CLEAR_BASE_3: u32 = 0x1579D;
const TPC_PACKAGE_A_BASE_4: u32 = 0x18F5F;
const TPC_PACKAGE_B_SET_BASE_4: u32 = 0x18FA5;
const TPC_PACKAGE_B_CLEAR_BASE_4: u32 = 0x18FEB;

/// The container file ids a Tpc LOADEXTSCHEDULER package names: A is
/// attached with resource tag 1, B with tag 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TpcMotionPackages {
    pub a: u32,
    pub b_set: u32,
    pub b_clear: u32,
}

/// The tag-2 container a CIB waist byte selects out of a Tpc package's two
/// candidates: 1 takes `b_set`, 2..=0x7F takes `b_clear`, 0 or >= 0x80
/// (including no CIB) loads A only - the re-read after container A lands skips
/// B when the signed byte is <= 0 (FFXiMain.dll ReadTpcEventMotionRes @rva
/// 0xD2230).
pub fn tpc_b_for_waist(b_set: u32, b_clear: u32, waist: u8) -> Option<u32> {
    match waist {
        1 => Some(b_set),
        2..=0x7F => Some(b_clear),
        _ => None,
    }
}

/// The container file ids a 0x66 Tpc motion package `param` names, or `None`
/// at or past the package limit, where retail logs and loads nothing
/// (FFXiMain.dll ReadTpcEventMotionRes @rva 0xD2230).
pub fn tpc_motion_packages(param: i32) -> Option<TpcMotionPackages> {
    let val = param as u32;
    if val >= TPC_PACKAGE_OUT_OF_RANGE {
        return None;
    }
    let (v, a_base, b_set_base, b_clear_base) = if val < TPC_PACKAGE_BAND_2 {
        (
            val,
            TPC_PACKAGE_A_BASE_1,
            TPC_PACKAGE_B_SET_BASE_1,
            TPC_PACKAGE_B_CLEAR_BASE_1,
        )
    } else if val < TPC_PACKAGE_BAND_3 {
        (
            val - TPC_PACKAGE_BAND_2,
            TPC_PACKAGE_A_BASE_2,
            TPC_PACKAGE_B_SET_BASE_2,
            TPC_PACKAGE_B_CLEAR_BASE_2,
        )
    } else if val < TPC_PACKAGE_BAND_4 {
        (
            val - TPC_PACKAGE_BAND_3,
            TPC_PACKAGE_A_BASE_3,
            TPC_PACKAGE_B_SET_BASE_3,
            TPC_PACKAGE_B_CLEAR_BASE_3,
        )
    } else {
        (
            val - TPC_PACKAGE_BAND_4,
            TPC_PACKAGE_A_BASE_4,
            TPC_PACKAGE_B_SET_BASE_4,
            TPC_PACKAGE_B_CLEAR_BASE_4,
        )
    };
    Some(TpcMotionPackages {
        a: v + a_base,
        b_set: v + b_set_base,
        b_clear: v + b_clear_base,
    })
}

/// The motion resource a LOADEXTSCHEDULER cue loads before playing its key
/// (research/XiEvents/OpCodes/0x005B.md, 0x0066.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtSchedulerMotion {
    /// LOADEXTSCHEDULER: the event motion resource a single DAT file id names.
    Event(u32),
    /// The Tpc form in range: container A (resource tag 1) and the two B
    /// candidates (resource tag 2); the host picks between them from the
    /// actor's CIB waist byte, which this VM does not carry.
    Tpc(TpcMotionPackages),
}

/// The LOADEXTSCHEDULER "no action" key: retail loads the motion resource and
/// skips SetAction when the key is zero or these bytes.
pub const NO_ACTION_KEY: FourCc = *b"xxxx";

/// One staging effect the running event asked for. Emitted in execution order.
/// Not `Eq`: [`EventCue::ActorMove`] carries a float MoveTime budget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventCue {
    /// 0x2C SCHEDULOR: play action `key` on `actor1`, with `actor2` as the
    /// action's partner (research/XiEvents/OpCodes/0x002C.md).
    ActorMotion {
        actor1: ActorLookup,
        actor2: ActorLookup,
        key: FourCc,
    },
    /// 0x6E EMOT / 0x63 PLAYANIM: play the emote animation `emote_id` on
    /// `actor`, `param` the emote's variant selector (salute nation, …).
    /// `emote_id` is the low byte and `param` the high byte of the operand's
    /// work value (research/XiEvents/OpCodes/0x006E.md, 0x0063.md).
    Emote {
        actor: ActorLookup,
        emote_id: u16,
        param: u16,
    },
    /// 0x45 LOADEVENTSCHEDULER2: run scheduler `tag` out of DAT file `dat_id`
    /// over the two actors (research/XiEvents/OpCodes/0x0045.md). `duration` is
    /// the authored override, [`SCHEDULER_DURATION_FROM_DAT`] for none.
    Scheduler {
        dat_id: u32,
        actor1: ActorLookup,
        actor2: ActorLookup,
        tag: FourCc,
        duration: u16,
    },
    /// 0x5B/0x66 LOADEXTSCHEDULER: load the motion resource into `actor1`'s
    /// skeleton, then play action `key` on it with `actor2` as partner
    /// (research/XiEvents/OpCodes/0x005B.md, 0x0066.md). `motion` is `None`
    /// for the 0x66 out-of-range package, where retail logs and loads nothing
    /// and the host plays `key` on the actor's own resources.
    ExtScheduler {
        motion: Option<ExtSchedulerMotion>,
        actor1: ActorLookup,
        actor2: ActorLookup,
        key: FourCc,
    },
    /// 0x2D MAPSCHEDULOR: start the zone-level routine `key` out of the current
    /// zone's own model DAT over the two actors, waited on by 0x54
    /// (research/XiEvents/OpCodes/0x002D.md). The host arms that wait's hold
    /// from the routine's authored length in the file the key resolved in.
    ZoneScheduler {
        key: FourCc,
        actor1: ActorLookup,
        actor2: ActorLookup,
    },
    /// 0x4E EVENTHIDE: set/clear the target's event-hide render flag
    /// (research/XiEvents/OpCodes/0x004E.md).
    ActorHide { target: ActorLookup, hide: bool },
    /// 0x6C TRANSPAR: fade the target's alpha to `end_alpha` (a 0..=255 byte,
    /// the work(5) operand) over `duration_frames` frames (the work(7)
    /// operand, 0 read as 1), parking the script for that fade
    /// (research/XiEvents/OpCodes/0x006C.md).
    Transpar {
        actor: ActorLookup,
        end_alpha: i32,
        duration_frames: i32,
    },
    /// 0x46 DEFCAMERA: take the camera (and the cutscene HUD) away from the
    /// player, or give it back (research/XiEvents/OpCodes/0x0046.md). Retail's
    /// restore reads saved global camera state, so the cue carries none.
    CameraLock { lock: bool },
    /// 0x38: write the lower word of retail's `CliEventModeLocal` — the work
    /// operand's high byte with 0x20 forced (the base cinematic bit the
    /// handler always sets, so every authored value keeps it). While it holds,
    /// the client hides the local player model and the HUD pieces and lets the
    /// event drive the camera; the event end clears the flag
    /// (research/XiEvents/OpCodes/0x0038.md).
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
    /// 0x00C9.md). 0x77 sets the hour on the current day at minute zero; 0xA9
    /// zeros the local time first, so it jumps the whole date to Vana day
    /// `7 * work[1]` at 00:30.
    ClockHold {
        stop: bool,
        hour: Option<u32>,
        minute: u8,
        day_from_epoch: Option<u32>,
    },
    /// 0x5D MUSICVOLUME: ease the playing track to volume table index `volume`
    /// over `fade_frames` (research/XiEvents/OpCodes/0x005D.md).
    MusicVolume { volume: u8, fade_frames: u16 },
    /// 0x5C MUSIC: set BGM slot `slot`'s song to `track` and its start volume
    /// to `volume` (the 0x00-0x07 band starts at full, 127; the 0x80-0x87 band
    /// starts at the authored value). The slot indexes retail's `PTR_MusicSongIds`
    /// table, the same table the BGM slot layout reads
    /// (research/XiEvents/OpCodes/0x005C.md).
    MusicSong { slot: u8, track: u16, volume: u8 },
    /// 0x69/0x6A SET/CHANGE sound volume: set the named retail sound types
    /// (the `mask` bits) to `volume` over `fade_frames`
    /// (research/XiEvents/OpCodes/0x0069.md, 0x006A.md).
    SoundVolume {
        mask: u8,
        volume: u8,
        fade_frames: u16,
    },
    /// 0x7E CHOCOBO/MOUNT: put the target on or off a mount by writing its
    /// `StatusEvent` (research/XiEvents/OpCodes/0x007E.md). `mount_id` is
    /// carried only by the non-chocobo mount cases.
    Mount {
        target: ActorLookup,
        status_event: u8,
        mount_id: Option<u16>,
    },
    /// 0x1F MOVE case 0 on a non-player actor: walk the event entity to `goal`
    /// at `speed` (research/XiEvents/OpCodes/0x001F.md). The host arms the
    /// arrival hold from its own distance and speed; the VM never measures it.
    /// `max_time` is 0x31 SMOVE's MoveTime budget in seconds: when set and
    /// shorter than the distance-derived length, the host caps the hold to it
    /// (research/XiEvents/OpCodes/0x0031.md).
    ActorMove {
        actor: ActorLookup,
        goal: EventPosition,
        /// Raw MainSpeed operand of the trigger packet; the host scales it
        /// with [`crate::vm::scene::EVENT_SPEED_SCALE`].
        speed: i32,
        /// 0x31 SMOVE's MoveTime budget in seconds; `None` for 0x1F.
        max_time: Option<f32>,
    },
    /// 0x37 on a non-player actor: set the event entity's position (teleport,
    /// no hold; research/XiEvents/OpCodes/0x0037.md).
    ActorPlace {
        actor: ActorLookup,
        position: EventPosition,
    },
    /// 0x39 on a non-player actor: set the event entity's facing from its work
    /// operand (research/XiEvents/OpCodes/0x0039.md).
    ActorFace { actor: ActorLookup, heading: i32 },
    /// 0x4A DTURA, 0x79 lookat case 0 and the motion half of 0x1E
    /// look-and-talk: turn `actor` toward `target`
    /// (research/XiEvents/OpCodes/0x004A.md, 0x0079.md, 0x001E.md).
    ActorLookAt {
        actor: ActorLookup,
        target: ActorLookup,
    },
    /// 0x5E / 0x6B stop action: kill the current action on `actor` and return
    /// it to idle; `key` names the routine slot to clear when the operand is a
    /// nonzero tag (research/XiEvents/OpCodes/0x005E.md, 0x006B.md).
    ActorStopAction {
        actor: ActorLookup,
        key: Option<FourCc>,
    },
    /// 0xB5 case 0: set the event entity's display name to `name`, the work
    /// string its operand selects (research/XiEvents/OpCodes/0x00B5.md). That
    /// string is filled by 0xB4 case 0 (an inline literal) or case 1 (the
    /// s2c 0x005D PENDINGSTR table entry its work operand selects).
    EntityName { actor: ActorLookup, name: [u8; 16] },
    /// 0xC8 MAP_TUTORIAL: open the map window on zone `map_id`; `tutorial` is
    /// the LOBYTE of the third work operand (research/XiEvents/OpCodes/0x00C8.md).
    MapOpen { map_id: i32, tutorial: bool },
    /// 0x8B MAP_MARKER: place a named marker on the player's map at
    /// milli-unit coordinates; `name` is the raw 16-byte field with retail's
    /// underscore-to-space rewrite already applied
    /// (research/XiEvents/OpCodes/0x008B.md).
    MapMarker {
        map_id: i32,
        x_milli: i32,
        y_milli: i32,
        name: [u8; 16],
    },
    /// 0x8A CLOSE_MAP: close the map window (research/XiEvents/OpCodes/0x008A.md).
    MapClose,
}

impl EventCue {
    pub(crate) fn resolve_event_actor(self, actor: ActorLookup) -> Self {
        let resolve = |target: ActorLookup| {
            if target.is_event_entity() {
                actor
            } else {
                target
            }
        };
        match self {
            Self::ActorMotion {
                actor1,
                actor2,
                key,
            } => Self::ActorMotion {
                actor1: resolve(actor1),
                actor2: resolve(actor2),
                key,
            },
            Self::Emote {
                actor,
                emote_id,
                param,
            } => Self::Emote {
                actor: resolve(actor),
                emote_id,
                param,
            },
            Self::Scheduler {
                dat_id,
                actor1,
                actor2,
                tag,
                duration,
            } => Self::Scheduler {
                dat_id,
                actor1: resolve(actor1),
                actor2: resolve(actor2),
                tag,
                duration,
            },
            Self::ExtScheduler {
                motion,
                actor1,
                actor2,
                key,
            } => Self::ExtScheduler {
                motion,
                actor1: resolve(actor1),
                actor2: resolve(actor2),
                key,
            },
            Self::ZoneScheduler {
                key,
                actor1,
                actor2,
            } => Self::ZoneScheduler {
                key,
                actor1: resolve(actor1),
                actor2: resolve(actor2),
            },
            Self::ActorHide { target, hide } => Self::ActorHide {
                target: resolve(target),
                hide,
            },
            Self::Transpar {
                actor,
                end_alpha,
                duration_frames,
            } => Self::Transpar {
                actor: resolve(actor),
                end_alpha,
                duration_frames,
            },
            Self::Mount {
                target,
                status_event,
                mount_id,
            } => Self::Mount {
                target: resolve(target),
                status_event,
                mount_id,
            },
            Self::ActorMove {
                actor,
                goal,
                speed,
                max_time,
            } => Self::ActorMove {
                actor: resolve(actor),
                goal,
                speed,
                max_time,
            },
            Self::ActorPlace { actor, position } => Self::ActorPlace {
                actor: resolve(actor),
                position,
            },
            Self::ActorFace { actor, heading } => Self::ActorFace {
                actor: resolve(actor),
                heading,
            },
            Self::ActorLookAt { actor, target } => Self::ActorLookAt {
                actor: resolve(actor),
                target: resolve(target),
            },
            Self::ActorStopAction { actor, key } => Self::ActorStopAction {
                actor: resolve(actor),
                key,
            },
            Self::EntityName { actor, name } => Self::EntityName {
                actor: resolve(actor),
                name,
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Band bases read back off FFXiMain.dll independently of the consts above:
    // the band-edge tests are only worth running while their expected values
    // come from a second source, so they are pinned here rather than imported.
    const EVENT_MOTION_BASE_0_PINNED: u32 = 32104;
    const EVENT_MOTION_BASE_1_PINNED: u32 = 49135;
    const EVENT_MOTION_BASE_2_PINNED: u32 = 56345;
    const EVENT_MOTION_BASE_3_PINNED: u32 = 59739;
    const EVENT_MOTION_BASE_4_PINNED: u32 = 66339;
    const EVENT_MOTION_BAND_4_PINNED: u32 = 3072;
    const TPC_A_BASE_1_PINNED: u32 = 32712;
    const TPC_B_SET_BASE_1_PINNED: u32 = 32782;
    const TPC_B_CLEAR_BASE_1_PINNED: u32 = 32852;
    const TPC_A_BASE_2_PINNED: u32 = 61241;
    const TPC_B_SET_BASE_2_PINNED: u32 = 61311;
    const TPC_B_CLEAR_BASE_2_PINNED: u32 = 61381;
    const TPC_A_BASE_3_PINNED: u32 = 87825;
    const TPC_B_SET_BASE_3_PINNED: u32 = 87895;
    const TPC_B_CLEAR_BASE_3_PINNED: u32 = 87965;
    const TPC_A_BASE_4_PINNED: u32 = 102239;
    const TPC_B_SET_BASE_4_PINNED: u32 = 102309;
    const TPC_B_CLEAR_BASE_4_PINNED: u32 = 102379;
    // The fold offsets of FUNC_DatIdHelper stay literal here: the band-edge
    // tests only prove band selection while the offsets come from a second
    // source (research/XiEvents/OpCodes/0x0045.md).
    const MID_BAND_OFFSET_PINNED: i32 = 25937;
    const HIGH_BAND_OFFSET_PINNED: i32 = 39643;

    /// The fade pair's DAT id is the one the 0x45 base plus its authored work
    /// operand resolves to; both consts must keep agreeing.
    #[test]
    fn fade_scheduler_dat_id_is_the_base_plus_its_authored_operand() {
        const FADE_WORK_OPERAND: i32 = 200;
        assert_eq!(
            SCHEDULER_DAT_ID_BASE as i32 + dat_id_helper(FADE_WORK_OPERAND),
            SCHEDULER_FADE_DAT_ID as i32
        );
    }

    #[test]
    fn fade_tags_are_the_authored_ascii_fourccs() {
        assert_eq!(&SCHEDULER_TAG_FADE_OUT, b"fdo0");
        assert_eq!(&SCHEDULER_TAG_FADE_IN, b"fdi0");
        for tag in [SCHEDULER_TAG_FADE_OUT, SCHEDULER_TAG_FADE_IN] {
            assert!(tag.iter().all(|b| (0x20..0x7F).contains(b)), "{tag:?}");
        }
    }

    #[test]
    fn dat_id_helper_folds_at_its_two_band_edges() {
        assert_eq!(dat_id_helper(0), 0);
        assert_eq!(dat_id_helper(299), 299);
        assert_eq!(dat_id_helper(300), 300 + MID_BAND_OFFSET_PINNED);
        assert_eq!(dat_id_helper(599), 599 + MID_BAND_OFFSET_PINNED);
        assert_eq!(dat_id_helper(600), 600 + HIGH_BAND_OFFSET_PINNED);
    }

    /// Each band edge lands on the next base exactly once; the value just
    /// below an edge stays in the current band.
    #[test]
    fn event_motion_dat_id_picks_its_base_at_each_band_edge() {
        let band_4 = EVENT_MOTION_BAND_4_PINNED;
        assert_eq!(event_motion_dat_id(0), EVENT_MOTION_BASE_0_PINNED);
        assert_eq!(event_motion_dat_id(511), EVENT_MOTION_BASE_0_PINNED + 511);
        assert_eq!(event_motion_dat_id(512), EVENT_MOTION_BASE_1_PINNED + 512);
        assert_eq!(event_motion_dat_id(1023), EVENT_MOTION_BASE_1_PINNED + 1023);
        assert_eq!(event_motion_dat_id(1024), EVENT_MOTION_BASE_2_PINNED + 1024);
        assert_eq!(event_motion_dat_id(2047), EVENT_MOTION_BASE_2_PINNED + 2047);
        assert_eq!(event_motion_dat_id(2048), EVENT_MOTION_BASE_3_PINNED + 2048);
        assert_eq!(event_motion_dat_id(3071), EVENT_MOTION_BASE_3_PINNED + 3071);
        assert_eq!(
            event_motion_dat_id(band_4 as i32),
            EVENT_MOTION_BASE_4_PINNED + band_4
        );
    }

    /// Each band edge lands on the next base exactly once; the value just
    /// below an edge stays in the current band; a negative operand is a huge
    /// unsigned package, out of range.
    #[test]
    fn tpc_motion_packages_picks_its_base_at_each_band_edge() {
        assert_eq!(
            tpc_motion_packages(0),
            Some(TpcMotionPackages {
                a: TPC_A_BASE_1_PINNED,
                b_set: TPC_B_SET_BASE_1_PINNED,
                b_clear: TPC_B_CLEAR_BASE_1_PINNED,
            })
        );
        assert_eq!(
            tpc_motion_packages(0x45),
            Some(TpcMotionPackages {
                a: TPC_A_BASE_1_PINNED + 0x45,
                b_set: TPC_B_SET_BASE_1_PINNED + 0x45,
                b_clear: TPC_B_CLEAR_BASE_1_PINNED + 0x45,
            })
        );
        assert_eq!(
            tpc_motion_packages(0x46),
            Some(TpcMotionPackages {
                a: TPC_A_BASE_2_PINNED,
                b_set: TPC_B_SET_BASE_2_PINNED,
                b_clear: TPC_B_CLEAR_BASE_2_PINNED,
            })
        );
        assert_eq!(
            tpc_motion_packages(0x8B),
            Some(TpcMotionPackages {
                a: TPC_A_BASE_2_PINNED + 0x45,
                b_set: TPC_B_SET_BASE_2_PINNED + 0x45,
                b_clear: TPC_B_CLEAR_BASE_2_PINNED + 0x45,
            })
        );
        assert_eq!(
            tpc_motion_packages(0x8C),
            Some(TpcMotionPackages {
                a: TPC_A_BASE_3_PINNED,
                b_set: TPC_B_SET_BASE_3_PINNED,
                b_clear: TPC_B_CLEAR_BASE_3_PINNED,
            })
        );
        assert_eq!(
            tpc_motion_packages(0xD1),
            Some(TpcMotionPackages {
                a: TPC_A_BASE_3_PINNED + 0x45,
                b_set: TPC_B_SET_BASE_3_PINNED + 0x45,
                b_clear: TPC_B_CLEAR_BASE_3_PINNED + 0x45,
            })
        );
        assert_eq!(
            tpc_motion_packages(0xD2),
            Some(TpcMotionPackages {
                a: TPC_A_BASE_4_PINNED,
                b_set: TPC_B_SET_BASE_4_PINNED,
                b_clear: TPC_B_CLEAR_BASE_4_PINNED,
            })
        );
        assert_eq!(
            tpc_motion_packages(0x117),
            Some(TpcMotionPackages {
                a: TPC_A_BASE_4_PINNED + 0x45,
                b_set: TPC_B_SET_BASE_4_PINNED + 0x45,
                b_clear: TPC_B_CLEAR_BASE_4_PINNED + 0x45,
            })
        );
        assert_eq!(tpc_motion_packages(0x118), None);
        assert_eq!(tpc_motion_packages(0x118 + 1), None);
        assert_eq!(tpc_motion_packages(-1), None);
    }

    /// Packages 20 (the Sandy opening scene) and 12, cross-checked against
    /// the install's DATs: 32732 holds tlk0 + thk1, 32724 holds kka0.
    #[test]
    fn tpc_motion_packages_hits_the_retail_anchors() {
        assert_eq!(
            tpc_motion_packages(20),
            Some(TpcMotionPackages {
                a: 32732,
                b_set: 32802,
                b_clear: 32872,
            })
        );
        assert_eq!(
            tpc_motion_packages(12),
            Some(TpcMotionPackages {
                a: 32724,
                b_set: 32794,
                b_clear: 32864,
            })
        );
    }

    #[test]
    fn tpc_b_for_waist_follows_the_retail_flag_rule() {
        let pkgs = tpc_motion_packages(20).unwrap();
        assert_eq!(
            tpc_b_for_waist(pkgs.b_set, pkgs.b_clear, 1),
            Some(pkgs.b_set)
        );
        assert_eq!(
            tpc_b_for_waist(pkgs.b_set, pkgs.b_clear, 2),
            Some(pkgs.b_clear)
        );
        assert_eq!(
            tpc_b_for_waist(pkgs.b_set, pkgs.b_clear, 0x7F),
            Some(pkgs.b_clear)
        );
        assert_eq!(tpc_b_for_waist(pkgs.b_set, pkgs.b_clear, 0), None);
        assert_eq!(tpc_b_for_waist(pkgs.b_set, pkgs.b_clear, 0x80), None);
        assert_eq!(tpc_b_for_waist(pkgs.b_set, pkgs.b_clear, 0xFF), None);
    }

    #[test]
    fn actor_lookup_separates_the_player_the_event_entity_and_server_ids() {
        assert!(ActorLookup::LOCAL_PLAYER.is_local_player());
        assert!(!ActorLookup::LOCAL_PLAYER.is_event_entity());
        assert!(ActorLookup(LOOKUP_LOCAL_PLAYER_A).is_local_player());
        assert!(ActorLookup(LOOKUP_LOCAL_PLAYER_C).is_local_player());

        assert!(ActorLookup::EVENT_ENTITY.is_event_entity());
        assert!(!ActorLookup::EVENT_ENTITY.is_local_player());
        assert_eq!(ActorLookup::EVENT_ENTITY.server_id(), None);

        let npc = ActorLookup(0x010E_6032);
        assert_eq!(npc.server_id(), Some(0x010E_6032));
        assert_eq!(npc.target_index(), Some(0x032));
        assert!(!npc.is_local_player() && !npc.is_event_entity());

        // No high byte and not reserved: the default handler's fallback.
        assert!(ActorLookup(0x0000_0042).is_event_entity());

        assert!(!ActorLookup::ZONE.is_local_player());
        assert!(!ActorLookup::ZONE.is_event_entity());
    }
}
