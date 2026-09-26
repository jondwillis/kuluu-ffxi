pub mod scene;

use std::sync::{Arc, Mutex};

use ffxi_dat::event_dat::EventBlock;

use crate::cue::{
    dat_id_helper, event_motion_dat_id, scheduler_twin_base, tpc_motion_packages, ActorLookup,
    EventCue, ExtSchedulerMotion, FourCc, EMOTE_ANIMATION_KEY, LOCAL_PLAYER_SCHEDULER_DAT_ID_BASE,
    MAGIC_DAT_ID_BASE, MAGIC_ROUTINE_TAG, MUSIC_VOLUME_MAX, NO_ACTION_KEY, SCHEDULER_DAT_ID_BASE,
    SCHEDULER_DURATION_FROM_DAT, STATUS_EVENT_CHOCOBO, STATUS_EVENT_DOOR_CLOSE,
    STATUS_EVENT_DOOR_CLOSE2, STATUS_EVENT_DOOR_OPEN, STATUS_EVENT_DOOR_OPEN2, STATUS_EVENT_IDLE,
    STATUS_EVENT_MOTION_BASE, STATUS_EVENT_MOUNT,
};
use crate::opcode_meta::{
    OPCODE_META, OP_ENTITYSPEED, OP_EVENTPOSSET, OP_ITEMINFO, OP_LOADROOM, OP_LOOKSET, OP_MENU,
    OP_MOVE, OP_NAMESET, OP_RENDERFLAG, OP_REQRESET, OP_STATUSSET, OP_STRINGOPS, OP_SUBSCHED,
    OP_WINDOW,
};

/// A message the VM asked to display: dialog string `message_id` from the zone
/// dialog DAT ([`ffxi_dat::dmsg::StringDat`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventMessage {
    pub message_id: u32,
    /// Entity whose name prefixes the line — retail's `EventMessDecodePutMoute`
    /// name argument, `MESCASNAMEINDEX`/`MESTARNAMEINDEX`. `None` for the
    /// speakerless message opcodes, which print through `EventMessDecodePut`
    /// with no name (research/XiEvents/OpCodes/0x0048.md, 0x0049.md).
    pub speaker_index: Option<u16>,
    /// The event's numeric parameters (`num[8]` from the 0x33/0x34 trigger
    /// packet), consumed by the dialog string's parameterized control codes:
    /// `{Num:N}` prints `params[N]`, `{Choice:N}[a/b/…]` selects alternative
    /// `params[N]`. Empty for a 0x32 trigger (it carries no parameters).
    pub params: Vec<i32>,
}

/// A choice menu the VM asked to present (0x24 QUERY). The selectable options
/// live inside dialog string `message_id` (split on its selection control
/// codes); `default_index` is the initial cursor. The host renders it and feeds
/// the result back via [`EventVm::select_choice`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventChoice {
    pub message_id: u32,
    pub speaker_index: u16,
    pub default_index: u32,
    /// Event numeric parameters — see [`EventMessage::params`].
    pub params: Vec<i32>,
}

/// A mid-event tag the VM has sent to the server and holds execution on until
/// the s2c ack (PENDINGNUM/PENDINGSTR) arrives. The two shapes are the c2s
/// packets retail's pending-tag functions build: EVENTEND for a send-tag,
/// EVENTENDXZY for a position tag (research/XiPackets/world/client/0x005B, 0x005C).
#[derive(Debug, Clone, PartialEq)]
pub enum PendingTag {
    /// `FUNC_SendPendingTag`: the client returns `Work_Zone[1]` as the 0x05B
    /// `EndPara` (research/XiPackets/world/client/0x005B).
    SendTag { end_para: u32 },
    /// `FUNC_SendPendingXzyTag`: the opcode-scaled position values and the
    /// heading already on the wire's 0..=255 scale. The opcode scales its
    /// work-slot raw values to radians (research/XiEvents/OpCodes/0x0047.md);
    /// LSB stores the c2s `dir` byte into `position_t.rotation`, whose
    /// documented radian conversion is `radianToRotation`
    /// (vendor/server/src/common/mmo.h, vendor/server/src/common/utils.cpp),
    /// so the tag carries the converted byte rather than the float.
    /// `end_para` is Work_Zone[1] at send time: the c2s packet's EndPara field
    /// (research/XiPackets/world/client/0x005C) and the server's OnEventUpdate
    /// result (vendor/server/src/map/packets/c2s/0x05c_eventendxzy.cpp).
    SendXzy {
        x: f32,
        y: f32,
        z: f32,
        dir: u8,
        end_para: u32,
    },
    /// `CTkDelivery_RequestEnterDeliveryMode`: the 0x04D PBX open request the
    /// 0xB2 case 1 sends, acked by the 0x04B DELI_OPEN/POST_OPEN result
    /// (research/XiEvents/OpCodes/0x00B2.md;
    /// vendor/server/src/map/packets/c2s/0x04d_pbx.cpp).
    DeliveryOpen,
    /// `FUNC_gcZoneSendQueSearch(0xEB)`: the header-only 0x0EB REQSUBMAPNUM
    /// request the 0xA6 case 0 sends; the 0x10E s2c's MapNum is its answer,
    /// and the server answers nothing when the char is not npc-locked, so the
    /// session's grace watchdog releases an unanswered hold
    /// (research/XiEvents/OpCodes/0x00A6.md;
    /// vendor/server/src/map/packets/c2s/0x0eb_reqsubmapnum.cpp).
    SubMapNum,
    /// `FUNC_gcZoneSendQueSearch(0x1B)`: the world-pass request the 0x87/0x88
    /// send cases arm, carrying the `Para` the c2s 0x01B FRIENDPASS carries
    /// (0x87: 0 begin / 2 begin-gold; 0x88: 1 confirm / 3 confirm-gold); the
    /// 0x059 s2c is its answer (research/XiEvents/OpCodes/0x0087.md, 0x0088.md;
    /// vendor/server/src/map/packets/c2s/0x01b_friendpass.cpp).
    FriendPass { para: u16 },
    /// `FUNC_gcZoneSendQueSearch(0x58)`: the crafting-support request the 0x8C
    /// send cases arm, carrying the c2s 0x058 RECIPE's Mode/skill/level/
    /// Param0..4 exactly as the case's retail pseudo code fills them; the
    /// 0x031 s2c is its answer (research/XiEvents/OpCodes/0x008C.md;
    /// vendor/server/src/map/packets/c2s/0x058_recipe.cpp).
    Recipe {
        mode: u16,
        skill: u16,
        level: u16,
        param0: u16,
        param1: u16,
        param2: u16,
        param3: u16,
        param4: u16,
    },
}

/// Outcome of running the VM until it next needs the host (one `XiEvent::EventIdle`
/// tick: opcodes execute until `RetFlag`). Not `Eq`: [`PendingTag::SendXzy`]
/// carries floats.
#[derive(Debug, Clone, PartialEq)]
pub enum StepResult {
    /// A message was shown (0x1D/0x2B/0x48/0x49/0xB0) and the VM is blocked on
    /// MESWAIT (0x23). The host displays it, then calls
    /// [`EventVm::dismiss_message`] + [`EventVm::step`].
    AwaitMessage(EventMessage),
    /// MESWAIT reached with no fresh message (a dialog is still open).
    AwaitMessageAck,
    /// A choice menu was presented (0x24) and the VM is blocked on QUERYWAIT
    /// (0x25). The host renders it, then calls [`EventVm::select_choice`] + step.
    AwaitChoice(EventChoice),
    /// The event ended (end opcode / return past the top of the jump stack).
    Done,
    /// The event was force-cancelled (MESWAIT saw an invalid open state).
    Cancelled,
    /// An opcode the VM does not implement and cannot safely skip (a jump, a
    /// yielding opcode, or a player-input poll it cannot answer — see
    /// [`crate::opcode_meta::is_input_wait`]); execution stops rather than
    /// desyncing `ExecPointer` or inventing the answer.
    Unimplemented(u8),
    /// The step burned [`OPCODE_BUDGET_PER_STEP`] without reaching a yield: a
    /// loop whose condition only an unmodelled opcode would have moved. The
    /// payload is whatever the VM was sitting on when the budget ran out, *not*
    /// a refusal — it names no work. Hosts treat this exactly like
    /// [`StepResult::Unimplemented`]; it is split off so the corpus sweep can
    /// rank real refusals without these drowning them (kuluu-cjct).
    Spun(u8),
    /// Blocked on a timed wait; the host runs its clock into [`EventVm::tick`]
    /// and steps again. Pure timers yield here, as do the WAIT* scheduler holds
    /// while a host-armed, DAT-bounded action still has frames left; those holds
    /// are bounded by the routine's authored length, so they cannot hang.
    Waiting,
    /// A mid-event tag was sent to the server (the send-tag or position-tag
    /// opcode) and execution is held on its case-1 poll until the s2c ack
    /// arrives; the host calls [`EventVm::ack_server`] when it does.
    AwaitServerAck(PendingTag),
}

/// Why the VM is not advancing right now, for the host's liveness check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Park {
    /// Running, or ended.
    None,
    /// A frame is displayed and waits on the player; never stale.
    Frame,
    /// A timed wait with units left; stale only if the host stops ticking it.
    TimedWait,
    /// A scheduler/move hold the host must release when the action finishes.
    Hold,
    /// A pending tag awaiting the s2c ack.
    ServerAck,
    /// Parked with nothing that can move it (a yield-forever, an unmodelled poll).
    Dead,
}

pub(crate) const OP_END: u8 = 0x00;
const OP_GOTO: u8 = 0x01;
const OP_IF: u8 = 0x02;
const OP_GET_STORE: u8 = 0x03;
const OP_SET_ONE: u8 = 0x05;
const OP_SET_ZERO: u8 = 0x06;
const OP_ADD: u8 = 0x07;
const OP_SUB: u8 = 0x08;
const OP_BIT_SET: u8 = 0x09;
const OP_BIT_CLEAR: u8 = 0x0A;
const OP_INC: u8 = 0x0B;
const OP_DEC: u8 = 0x0C;
const OP_AND: u8 = 0x0D;
const OP_OR: u8 = 0x0E;
const OP_XOR: u8 = 0x0F;
const OP_SHL: u8 = 0x10;
const OP_SHR: u8 = 0x11;
const OP_MUL: u8 = 0x14;
const OP_DIV: u8 = 0x15;
const OP_SWAP: u8 = 0x19;
const OP_BITARRAY_SET: u8 = 0x3C;
pub(crate) const OP_WAIT: u8 = 0x1C;
const OP_JUMP: u8 = 0x1A;
const OP_RETURN: u8 = 0x1B;
pub(crate) const OP_MESSAGE: u8 = 0x1D;
const OP_MESSAGE_ACTOR: u8 = 0x2B;
const OP_MESSAGE_UNNAMED: u8 = 0x48;
const OP_MESSAGE_UNNAMED_ACTOR: u8 = 0x49;
const OP_MESSAGE_ACTOR_PAIR: u8 = 0xB0;
const OP_EXECEND: u8 = 0x21;
const OP_EVENT_HIDE_SELF: u8 = 0x22;
const OP_LOCAL_MODE: u8 = 0x38;
// 0x0038's operand high byte and the bit the handler forces into it
// (research/XiEvents/OpCodes/0x0038.md).
const LOCAL_MODE_BYTE_MASK: i32 = 0xFF;
const LOCAL_MODE_FORCED_BIT: u16 = 0x20;
pub(crate) const OP_MESWAIT: u8 = 0x23;
// 0x42 clears CliEventCancelSetData/CliEventCancelFlag and 0x2E sets them:
// whether ESC may cancel this event. Cutscenes that lock you in disarm it in
// the prologue (event 503's master block runs 0x42 as its second opcode);
// interactive beats re-arm it (research/XiEvents/OpCodes/0x0042.md, 0x002E.md).
const OP_CANCEL_DISARM: u8 = 0x42;
const OP_CANCEL_ARM: u8 = 0x2E;
pub(crate) const OP_QUERY: u8 = 0x24;
pub(crate) const OP_QUERYWAIT: u8 = 0x25;
const OP_REQSET: u8 = 0x27;
const OP_REQSET_CHECKED: u8 = 0x28;
const OP_REQSET_PRIORITY: u8 = 0x29;
const OP_REQWAIT: u8 = 0x2A;
const OP_LOADEXTSCHEDULER: u8 = 0x5B;
const OP_LOADEXTSCHEDULER2: u8 = 0x66;
const OP_SCHEDULOR: u8 = 0x2C;
const OP_MAPSCHEDULOR: u8 = 0x2D;
const OP_LOADEVENTSCHEDULER2: u8 = 0x45;
const OP_MAGICSCHEDULOR: u8 = 0x73;
// The 0x45 scheduler twins: same layout and cue, each on its own DAT base
// (research/XiEvents/OpCodes/0x0062.md and kin).
const OP_SCHED_TWIN_62: u8 = 0x62;
const OP_SCHED_TWIN_9F: u8 = 0x9F;
const OP_SCHED_TWIN_BB: u8 = 0xBB;
const OP_SCHED_TWIN_C5: u8 = 0xC5;
const OP_SCHED_TWIN_CD: u8 = 0xCD;
const OP_SCHED_TWIN_D0: u8 = 0xD0;
const OP_SCHED_TWIN_D5: u8 = 0xD5;
// The 0x73 twin: the spell cast with a case byte, wider than 0x73 by that
// byte (research/XiEvents/OpCodes/0x00C4.md).
const OP_MAGIC_TWIN: u8 = 0xC4;
// The end-scheduler family: kill the tag-named action on actor1, actor2
// riding along (research/XiEvents/OpCodes/0x0050.md, 0x0051.md, 0x0052.md).
// 0x50/0x51 are 13 bytes; 0x52 and its seven twins are 15.
const OP_ENDSCHEDULOR: u8 = 0x50;
const OP_ENDMAPSCHEDULOR: u8 = 0x51;
const OP_ENDLOADSCHEDULER_MAIN: u8 = 0x52;
const OP_ENDLOADSCHED_TWIN_A1: u8 = 0xA1;
const OP_ENDLOADSCHED_TWIN_A3: u8 = 0xA3;
const OP_ENDLOADSCHED_TWIN_BD: u8 = 0xBD;
const OP_ENDLOADSCHED_TWIN_C7: u8 = 0xC7;
const OP_ENDLOADSCHED_TWIN_CF: u8 = 0xCF;
const OP_ENDLOADSCHED_TWIN_D2: u8 = 0xD2;
const OP_ENDLOADSCHED_TWIN_D7: u8 = 0xD7;
// The local-player scheduler (rank-up animations): work at +1, both actors the
// player, the `main` routine (research/XiEvents/OpCodes/0x007D.md).
const OP_LOCAL_PLAYER_SCHEDULER: u8 = 0x7D;
// Non-scene NPC choreography: the same cues the scene path (vm/scene.rs) emits
// when the event carries scene data. 0x1F itself is the sub-byte `OP_MOVE`.
const OP_MAIN_SPEED: u8 = 0x32;
// 0x31 SMOVE: 0x1F with a heading update and a MoveTime budget, on the
// non-scene path (research/XiEvents/OpCodes/0x0031.md).
const OP_SMOVE: u8 = 0x31;
// 0xDA: a batch motion loader beyond the 0x00..=0xD9 meta range. A 6-byte
// header followed by 28-byte records, each naming (actor1, actor2, key);
// the VM emits one ActorMotion cue per record and advances past them all.
// Single-site in the retail corpus (zone 112 event 68), so the layout is
// pinned by that block's bytes, not a doc.
const OP_DA: u8 = 0xDA;
// 0xDA record geometry: 6-byte header, then 28-byte records; within a record
// actor1 is the u32 at +8, actor2 at +12, the key 4cc at +16.
const OP_DA_HEADER: usize = 6;
const OP_DA_RECORD: usize = 28;
const OP_DA_ACTOR1: usize = 8;
const OP_DA_ACTOR2: usize = 12;
const OP_DA_KEY: usize = 16;
const OP_DA_MAX_RECORDS: usize = 64;
// 0x72 GETWEATHER: read the global forecast table into Work_Zone[2..5).
// Sub-byte: mode 0 is 4 bytes, mode 1 is 6 (research/XiEvents/OpCodes/0x0072.md).
const OP_GETWEATHER: u8 = 0x72;
// 0x82 RANGE_RECT: hit-test the event entity against the zone range rect named
// by the u32 at +1; a hit advances 7, a miss jumps to the u16 at +5
// (research/XiEvents/OpCodes/0x0082.md).
const OP_RANGE_RECT: u8 = 0x82;
// 0x87/0x88 WORLD PASS: the send cases arm the 0x01B FRIENDPASS request
// with the case's `Para` (0x87: 0 begin / 2 begin-gold; 0x88: 1 confirm /
// 3 confirm-gold) and hold on the 0x059 answer; case 1 yields until it
// lands. The pass number the 0x059 carries has no kuluu display, so the
// receipt only clears the await
// (research/XiEvents/OpCodes/0x0087.md, 0x0088.md).
const OP_FRIENDPASS_87: u8 = 0x87;
const OP_FRIENDPASS_88: u8 = 0x88;
// 0x8C RECIPE: the crafting-support cases fill the 0x058 RECIPE with their
// Mode and work-slot fields, hold on the 0x031 answer, and case 1 yields
// until it lands; retail's case 5 sends mode 5 without setting the flag, and
// case 5 has no LSB handler, so it advances without a send
// (research/XiEvents/OpCodes/0x008C.md).
const OP_RECIPE: u8 = 0x8C;
// 0xD4 MAP_QUERY: case 0 runs the 0x24 query helper and opens the current
// zone's map (type 6), parking on the answer; case 2 runs the helper without
// the map; cases 1/3/4/5 copy into the client's query window, which has no
// kuluu counterpart, so they advance by their width
// (research/XiEvents/OpCodes/0x00D4.md).
const OP_MAP_QUERY: u8 = 0xD4;
// 0xA7 waits on the server's response to the client's request: case 0 sends
// the pending tag (EndPara = Work_Zone[1]) and case 1 writes the result the
// ack carried into its work slot (research/XiEvents/OpCodes/0x00A7.md).
const OP_A7_WAIT: u8 = 0xA7;
// 0xA6 requests the event's sub-map number: case 0 sends the header-only
// 0x0EB REQSUBMAPNUM and holds on the 0x10E answer, case 1 yields until it
// lands, and case 2 writes the answered MapNum into its work slot
// (research/XiEvents/OpCodes/0x00A6.md).
const OP_A6_SUBMAP: u8 = 0xA6;
// 0xB2: mode 0 is a timed wait (WaitTime from work slot 1, +4 on expiry);
// mode 1 requests delivery mode (the 0x04D PBX open, +2 on the 0x04B ack)
// (research/XiEvents/OpCodes/0x00B2.md).
const OP_B2_DELIVERY: u8 = 0xB2;
// 0xB3 RANKING: the ranking-board cases. LSB has no ranking handler, so the
// read cases write zeros into the board's work slots (the board draws empty)
// and every case advances by its width; no packet is sent
// (research/XiEvents/OpCodes/0x00B3.md).
const OP_RANKING: u8 = 0xB3;
const OP_SET_FACING: u8 = 0x39;
const OP_YAW: u8 = 0x4B;
const OP_SET_EVENT_POS: u8 = 0x36;
const OP_SET_ACTOR_POS: u8 = 0xBA;
// 0x0020: writes retail's CliEventUcFlag — the player-control lock
// (research/XiEvents/OpCodes/0x0020.md).
const OP_PLAYER_CONTROL: u8 = 0x20;
const OP_DEFCAMERA: u8 = 0x46;
const OP_EVENTHIDE: u8 = 0x4E;
const OP_CLOSE_MAP: u8 = 0x8A;
const OP_OPEN_MAP: u8 = 0x89;
const OP_OPEN_MAP_PROPS: u8 = 0x8D;
const OP_MAP_MARKER: u8 = 0x8B;
const OP_MAP_ADD_MARK: u8 = 0xB8;
const OP_MAP_TUTORIAL: u8 = 0xC8;
const OP_HIDE_HUD: u8 = 0x67;
const OP_SHOW_HUD: u8 = 0x68;
const OP_STOP_CLOCK: u8 = 0x77;
const OP_RESTORE_CLOCK: u8 = 0x78;
const OP_MUSIC: u8 = 0x5C;
const OP_MUSICVOLUME: u8 = 0x5D;
const OP_SET_SOUND_VOLUME: u8 = 0x69;
const OP_CHANGE_SOUND_VOLUME: u8 = 0x6A;
const OP_SET_CLOCK_DATE: u8 = 0xA9;
const OP_ENABLE_TIMER: u8 = 0xC9;
const OP_WAITSCHEDULOR: u8 = 0x53;
const OP_WAITMAPSCHEDULOR: u8 = 0x54;
const OP_WAITLOADSCHEDULER: u8 = 0x55;
// 0x55's WAITLOADSCHEDULER twins: the same hold, each on its own scheduler
// DAT base (research/XiEvents/OpCodes/0x00A0.md, 0x00BC.md, 0x00C6.md,
// 0x00CE.md, 0x00D1.md, 0x00D6.md).
const OP_WAITLOADSCHED_TWIN_A0: u8 = 0xA0;
const OP_WAITLOADSCHED_TWIN_BC: u8 = 0xBC;
const OP_WAITLOADSCHED_TWIN_C6: u8 = 0xC6;
const OP_WAITLOADSCHED_TWIN_CE: u8 = 0xCE;
const OP_WAITLOADSCHED_TWIN_D1: u8 = 0xD1;
const OP_WAITLOADSCHED_TWIN_D6: u8 = 0xD6;
const OP_ACTOR_NOP: u8 = 0x56;
const OP_ZONE_READ_YIELD: u8 = 0x98;
const OP_ANIM_YIELD: u8 = 0x9B;
const OP_YIELD_FOREVER: u8 = 0x26;
const OP_ENTITY_VALID: u8 = 0x44;
const OP_KILL_LAST_ACTION: u8 = 0xC1;
const OP_CHOCOBO: u8 = 0x7E;
// The door status writes: the event entity's StatusEvent, gated on a
// Render.Flags0 bit no tier names (research/XiEvents/OpCodes/0x004C.md,
// 0x004D.md, 0x004F.md).
const OP_DOOR_OPEN: u8 = 0x4C;
const OP_DOOR_CLOSE: u8 = 0x4D;
const OP_STATUS_EVENT: u8 = 0x4F;
// The D_OPEN2/D_CLOSE2 writes: 0x4C/0x4D's twins on the second door status
// pair, the same gate and field (research/XiEvents/OpCodes/0x008E.md,
// 0x008F.md).
const OP_DOOR_OPEN2: u8 = 0x8E;
const OP_DOOR_CLOSE2: u8 = 0x8F;
// 0x90 writes the event-hide flag (the 0x4E bit, value 1) on the event
// entity, plus a Flags1 bit no tier names (research/XiEvents/OpCodes/0x0090.md).
const OP_EVENT_HIDE_ALWAYS: u8 = 0x90;
const OP_SETBITWORK: u8 = 0x40;
const OP_GETBITWORK: u8 = 0x41;
const OP_SENDTAG: u8 = 0x43;
/// The position-tag opcodes scale their work-slot raw values by this to get
/// the c2s float coordinates (research/XiEvents/OpCodes/0x0047.md).
const XZY_COORD_SCALE: f32 = 0.001;
/// s2c PENDINGNUM's num[8] lands in Work_Zone starting at this index
/// (research/XiPackets/world/server/0x005C GP_SERV_PENDINGNUM).
const PENDING_NUM_WORK_ZONE_BASE: usize = 2;
const OP_SLEEP: u8 = 0x6F;
const OP_TURNWAIT: u8 = 0x70;
const OP_LOADWAIT: u8 = 0x80;
const OP_TURNCHECK: u8 = 0x76;
const OP_ANIMWAIT: u8 = 0x99;
const OP_EMOT: u8 = 0x6E;
const OP_TRANSPAR: u8 = 0x6C;
const OP_MAPLOAD: u8 = 0x34;
const OP_MAPLOAD_KEEP: u8 = 0x35;
const OP_MUSICREADWAIT: u8 = 0x9A;
const OP_YIELD: u8 = 0x58;
const OP_PLAYANIM: u8 = 0x63;
const OP_BITTEST: u8 = 0x3E;
const OP_QUERYWAIT2: u8 = 0x7F;

// Advances the load/turn opcodes take when their entity does not resolve —
// retail's own `!GetActorIndex` / `!entity` early exit, which is the only
// path this actor-less VM can be on (research/XiEvents/OpCodes/0x0080.md,
// 0x0076.md).
const LOADWAIT_SIZE: usize = 5;
// 0x6C TRANSPAR's width: the fade parks the script for its authored length,
// then advances past itself (research/XiEvents/OpCodes/0x006C.md).
const TRANSPAR_SIZE: usize = 9;
/// 0x0034.md (and 0x0035.md, the same handler without the zone close) spreads
/// its zone load over three `EventIdle` ticks driven by two file-scope counters,
/// advancing only on the last; the net effect of the sequence is +3, and this VM
/// has no frame clock to spend the first two on.
const MAPLOAD_SIZE: usize = 3;
/// 0x0058.md is `ExecPointer++; RetFlag = 1`; 0x009A.md yields only while the
/// music server is mid-read, which nothing here triggers.
const YIELD_SIZE: usize = 1;
/// 0x0025.md / 0x007F.md: one byte, `ExecPointer += 1` on every exit that
/// advances (selection taken, or no talk window open).
const QUERYWAIT_SIZE: usize = 1;

// 0x003E BITTEST operand layout (research/XiEvents/OpCodes/0x003E.md): the bit
// index at +3 selects a work slot `bit >> 5` past the one named at +1, and the
// branch target at +5 is taken when the bit is clear.
const BIT_TEST_INDEX_OFS: usize = 3;
const BIT_TEST_WORD_OFS: usize = 1;
const BIT_TEST_TARGET_OFS: usize = 5;
const BIT_TEST_WORD_SHIFT: i32 = 5;
const BIT_TEST_BIT_MASK: i32 = 0x1F;

const IF_KIND_MASK: u8 = 0x0F;

const MESSAGE_OPEN_NONE: u8 = 0;
const MESSAGE_OPEN_AWAITING: u8 = 1;
// CliEventMessOpenFlag = 2 is the invalid-open state MESWAIT force-cancels on
// (research/XiEvents/OpCodes/0x0023.md).
const MESSAGE_OPEN_INVALID: u8 = 2;

// Operand offsets from the opcode byte, per research/XiEvents/OpCodes/*.md.
const MESSAGE_ID_OFS: usize = 1; // 0x001D, 0x0048
const ACTOR_LOOKUP_OFS: usize = 1; // 0x002B, 0x0049
const ACTOR_MESSAGE_ID_OFS: usize = 5; // 0x002B, 0x0049
const ACTOR_PAIR_STALL_FLAG_OFS: usize = 1; // 0x00B0
const ACTOR_PAIR_SPEAKER_OFS: usize = 2;
// The listener at +6 selects the mouth-animation entity; unread until the
// renderer models lip-sync (research/XiEvents/OpCodes/0x00B0.md).
const ACTOR_PAIR_MESSAGE_ID_OFS: usize = 10;

// Choreography operand offsets from the opcode byte, per
// research/XiEvents/OpCodes/*.md.
const SCHEDULOR_ACTOR1_OFS: usize = 1; // 0x002C
const SCHEDULOR_ACTOR2_OFS: usize = 5;
const SCHEDULOR_KEY_OFS: usize = 9;
const LOADEVENTSCHEDULER2_FILE_OFS: usize = 1; // 0x0045
const LOADEVENTSCHEDULER2_ACTOR1_OFS: usize = 3;
const LOADEVENTSCHEDULER2_ACTOR2_OFS: usize = 7;
const LOADEVENTSCHEDULER2_TAG_OFS: usize = 11;
const LOADEVENTSCHEDULER2_DURATION_OFS: usize = 15;
// 0x0073: work operand at +1 (`getworkofs(param1 + 1)`, param1 = 0), the two
// actor lookups at +3 and +7 (`eventgetcode2(3)`, `eventgetcode2(7 + param1)`),
// research/XiEvents/OpCodes/0x0073.md.
const MAGICSCHEDULOR_KEY_OFS: usize = 1;
const MAGICSCHEDULOR_ACTOR1_OFS: usize = 3;
const MAGICSCHEDULOR_ACTOR2_OFS: usize = 7;
// 0x0050/0x0051: actor1 at +1, the tag at +9 (research/XiEvents/OpCodes/
// 0x0050.md, 0x0051.md).
const ENDSCHEDULOR_ACTOR1_OFS: usize = 1;
const ENDSCHEDULOR_TAG_OFS: usize = 9;
// 0x0052 and its twins: actor1 at +3, the tag at +11; the work operand at +1
// selects the DAT, which the stop cue does not carry
// (research/XiEvents/OpCodes/0x0052.md).
const ENDLOADSCHED_ACTOR1_OFS: usize = 3;
const ENDLOADSCHED_TAG_OFS: usize = 11;
/// The stop family's tag operand values that name no routine slot: zero, and
/// the four spaces retail writes over a cleared tag
/// (research/XiEvents/OpCodes/0x005E.md `Unknown0001 = 0x20202020`).
const STOP_TAG_ZERO: FourCc = [0, 0, 0, 0];
const STOP_TAG_SPACES: FourCc = *b"    ";

/// The stop family's tag operand as the cue's `key`.
fn stop_action_key(tag: FourCc) -> Option<FourCc> {
    (tag != STOP_TAG_ZERO && tag != STOP_TAG_SPACES).then_some(tag)
}
// 0x00C4: the 0x73 sub-handler with param1 = 1 — the case byte at +1, the key
// work at +2 (`getworkofs(param1 + 1)`), actor1 at +3, actor2 at +8
// (`eventgetcode2(7 + param1)`), and the advance is `11 + param1` = 12
// (research/XiEvents/OpCodes/0x00C4.md, 0x0073.md).
const MAGIC_TWIN_CASE_OFS: usize = 1;
const MAGIC_TWIN_KEY_OFS: usize = 2;
const MAGIC_TWIN_ACTOR1_OFS: usize = 3;
const MAGIC_TWIN_ACTOR2_OFS: usize = 8;
const MAGIC_TWIN_SIZE: usize = 12;
// 0x007D: the work operand at +1 (research/XiEvents/OpCodes/0x007D.md).
const LOCAL_PLAYER_SCHEDULER_FILE_OFS: usize = 1;
// 0x0032 MainSpeed: the speed operand at +1 (research/XiEvents/OpCodes/0x0032.md).
const MAIN_SPEED_OFS: usize = 1;
// 0x0039 SetFacing: the heading operand at +1 (research/XiEvents/OpCodes/0x0039.md).
const SET_FACING_OFS: usize = 1;
// 0x004B yaw: the named actor at +1, the heading at +5
// (research/XiEvents/OpCodes/0x004B.md).
const YAW_ACTOR_OFS: usize = 1;
const YAW_HEADING_OFS: usize = 5;
// 0x0036 / 0x00BA position: x, z, y at +1/+3/+5 (0x36) and +5/+7/+9 with the
// heading at +11 (0xBA); the goal of 0x1F case 0 sits at +2/+4/+6
// (research/XiEvents/OpCodes/0x0036.md, 0x00BA.md, 0x001F.md).
const SET_EVENT_POS_X_OFS: usize = 1;
const SET_ACTOR_POS_ACTOR_OFS: usize = 1;
const SET_ACTOR_POS_X_OFS: usize = 5;
const MOVE_GOAL_X_OFS: usize = 2;
const LOADEXTSCHEDULER_FILE_OFS: usize = 1; // 0x005B / 0x0066
const LOADEXTSCHEDULER_ACTOR1_OFS: usize = 3;
const LOADEXTSCHEDULER_ACTOR2_OFS: usize = 7;
const LOADEXTSCHEDULER_KEY_OFS: usize = 11;
// The WAIT* family's host-armed hold matches on (actor1, key) only; retail's
// IsMovingAction also takes actor2, but the partner does not change which armed
// hold a wait parks on (research/XiEvents/OpCodes/0x0053.md, 0x0054.md).
const WAITSCHEDULOR_ACTOR1_OFS: usize = 1;
const WAITSCHEDULOR_KEY_OFS: usize = 9;
const WAITLOADSCHEDULER_ACTOR1_OFS: usize = 3;
const WAITLOADSCHEDULER_KEY_OFS: usize = 11;
// 0x0044: the work operand at +1 names the entity to test, the else-target
// at +3 (research/XiEvents/OpCodes/0x0044.md).
const ENTITY_VALID_ID_OFS: usize = 1;
const ENTITY_VALID_TARGET_OFS: usize = 3;
// 0x0056: the actor at +1, read and discarded (research/XiEvents/OpCodes/0x0056.md).
const ACTOR_NOP_ACTOR_OFS: usize = 1;
// 0x00C1: the actor at +1 (research/XiEvents/OpCodes/0x00C1.md).
const KILL_LAST_ACTION_ACTOR_OFS: usize = 1;
const MAPSCHEDULOR_KEY_OFS: usize = 9; // 0x002D, same layout as the WAIT family
const MAPSCHEDULOR_ACTOR2_OFS: usize = 5; // 0x002D partner slot of that layout
                                          // 0x006E EMOT: actor lookup at +1, the work value (emote id low byte, variant
                                          // high byte) at +5 (research/XiEvents/OpCodes/0x006E.md).
const EMOT_ACTOR_OFS: usize = 1;
const EMOT_VALUE_OFS: usize = 5;
// 0x006E's work value: emote id in the low byte, variant in the high byte
// (research/XiEvents/OpCodes/0x006E.md).
const EMOTE_VALUE_BYTE_MASK: i32 = 0xFF;
const EMOTE_VALUE_PARAM_SHIFT: u32 = 8;
// 0x0063 PLAYANIM: the event entity's emote from the work value at +1
// (research/XiEvents/OpCodes/0x0063.md).
const PLAYANIM_VALUE_OFS: usize = 1;
// 0x0099 ANIMWAIT: the actor lookup at +1 (research/XiEvents/OpCodes/0x0099.md).
const ANIMWAIT_ACTOR_OFS: usize = 1;
// 0x0020: the flag byte at +1 (research/XiEvents/OpCodes/0x0020.md).
const PLAYER_CONTROL_FLAG_OFS: usize = 1;
const DEFCAMERA_CASE_OFS: usize = 1; // 0x0046
const DEFCAMERA_CASE_UNLOCK: u8 = 0;
const DEFCAMERA_CASE_LOCK: u8 = 1;
const EVENTHIDE_FLAG_OFS: usize = 1; // 0x004E
const EVENTHIDE_FLAG_MASK: u8 = 1;
const EVENTHIDE_TARGET_OFS: usize = 2;
// 0x006C TRANSPAR: the actor lookup at +1, the destination alpha byte at +5,
// and the fade length in frames at +7 (research/XiEvents/OpCodes/0x006C.md).
const TRANSPAR_ACTOR_OFS: usize = 1;
const TRANSPAR_ALPHA_OFS: usize = 5;
const TRANSPAR_TIME_OFS: usize = 7;
const MUSICVOLUME_LEVEL_OFS: usize = 1; // 0x005D
const MUSICVOLUME_FADE_OFS: usize = 3;
// 0x005C: the low band (0x00-0x07) is 4 bytes, the 0x80-0x87 and 0xA0/0xA1
// bands are 6; the song id is the +2 work selector in both song bands
// (research/XiEvents/OpCodes/0x005C.md).
const MUSIC_SONG_TRACK_OFS: usize = 2;
const MUSIC_SONG_VOLUME_OFS: usize = 4;
// 0x005C's 0x80-0x87 band: the slot is the sub-byte's low three bits
// (research/XiEvents/OpCodes/0x005C.md).
const MUSIC_SONG_SLOT_MASK: u8 = 0x07;
/// 0x77's hour operand (research/XiEvents/OpCodes/0x0077.md); its weather
/// half is server-driven here, so it has no cue.
const STOP_CLOCK_HOUR_OFS: usize = 1;
/// `OP_STOP_CLOCK`'s "no time change" sentinel for the hour operand.
const STOP_CLOCK_NO_HOUR: i32 = 255;
/// 0x69's on/off flag byte (0 -> full volume, non-zero -> mute) and its
/// sound-type mask at +2 (research/XiEvents/OpCodes/0x0069.md).
const SET_SOUND_FLAG_OFS: usize = 1;
const SET_SOUND_MASK_OFS: usize = 2;
/// 0x6A's volume (work[1] * 0.001), fade frames (work[3]) and sound-type mask
/// (work[5]) (research/XiEvents/OpCodes/0x006A.md).
const CHANGE_SOUND_LEVEL_OFS: usize = 1;
const CHANGE_SOUND_FADE_OFS: usize = 3;
const CHANGE_SOUND_MASK_OFS: usize = 5;
/// 0xA9's day operand: the clock jumps to Vana day `7 * work[1]` at 00:30
/// (research/XiEvents/OpCodes/0x00A9.md).
const SET_CLOCK_DATE_DAY_OFS: usize = 1;
/// 0xA9's authored minute and hour (the local time is zeroed before the day
/// jump, so the clock lands at 00:30; research/XiEvents/OpCodes/0x00A9.md).
const SET_CLOCK_DATE_MINUTE: u8 = 30;
const SET_CLOCK_DATE_HOUR: u32 = 0;
const MAP_OPEN_ID_OFS: usize = 1; // 0x00C8
const MAP_OPEN_TUTORIAL_OFS: usize = 5; // 0x00C8, LOBYTE is the bool
const MAP_MARKER_ID_OFS: usize = 1; // 0x008B
const MAP_MARKER_X_OFS: usize = 5; // 0x008B
const MAP_MARKER_Y_OFS: usize = 7; // 0x008B
const MAP_MARKER_NAME_OFS: usize = 9; // 0x008B, 16 bytes
const OPEN_MAP_ID_OFS: usize = 1; // 0x0089, 0x008D
const MAP_ADD_MARK_ID_OFS: usize = 1; // 0x00B8
const MAP_ADD_MARK_X_OFS: usize = 7; // 0x00B8
const MAP_ADD_MARK_Y_OFS: usize = 9; // 0x00B8
const MAP_ADD_MARK_NAME_OFS: usize = 11; // 0x00B8, 16 bytes
const CHOCOBO_CASE_OFS: usize = 1; // 0x007E
const CHOCOBO_TARGET_OFS: usize = 2;
const CHOCOBO_MOUNT_ID_OFS: usize = 6;
/// 0x7E cases that write a `StatusEvent`, by the value they write
/// (research/XiEvents/OpCodes/0x007E.md). Case 2 is deliberately absent — see
/// its arm in [`EventVm::step`] — as is case 4, which writes nothing.
const CHOCOBO_CASES_IDLE: [u8; 2] = [0, 5];
const CHOCOBO_CASES_CHOCOBO: [u8; 3] = [1, 3, 6];
const CHOCOBO_CASE_MOUNT: u8 = 7;
const CHOCOBO_CASE_UNMOUNT: u8 = 8;
/// `entity->MountId = getworkofs(6) + 1` — the id is stored biased by one.
const CHOCOBO_MOUNT_ID_BIAS: u16 = 1;
const CHOCOBO_UNMOUNT_ID: u16 = 0;
/// 0x4F's work operand, the value added to `STATUS_EVENT_MOTION_BASE`
/// (research/XiEvents/OpCodes/0x004F.md).
const STATUS_EVENT_VALUE_OFS: usize = 1;

const WORK_LOCAL_LEN: usize = 80;
// `XiEvent::setworkstrofs` refuses string writes at slot 64 and up: a 16-byte
// store must stay inside the WorkLocal table (research/XiEvents/Event VM
// Functions.md; the int view bounds the same table at 80 slots).
const WORK_STR_WRITE_LEN: usize = 64;
const WORK_ZONE_LEN: usize = 96;
const WORK_ZONE_BASE: u32 = 4096;
/// The trigger packet's num fields land in Work_Zone at this offset: Selbina's
/// Lucia event 221 reads num[0] as Work_Zone[2] and Southern San d'Oria event
/// 599 reads num[1..2] as Work_Zone[3..4], both in the retail event DATs.
const EVENT_PARAM_WORK_BASE: usize = 2;
const EVENT_PARAM_COUNT: usize = 8;
const JUMP_STACK_LEN: usize = 8;
// References-table index marker; low bits index it (XiEvents Event VM Functions.md).
const REFERENCE_FLAG: u32 = 0x8000;
const REFERENCE_INDEX_MASK: u32 = 0x7FFF;
/// QUERYWAIT stores this in `Work_Zone[0]` when the player cancels the menu.
const CHOICE_CANCELLED: u32 = 254;
/// QUERYWAIT2 (0x7F) stores 255 for the same cancel and runs on
/// (research/XiEvents/OpCodes/0x007F.md).
const CHOICE_CANCELLED_QUERYWAIT2: u32 = 255;
/// Opcodes one [`EventVm::step`] may run before it gives up — see the check
/// itself. Far above any authored run between yields, so it only ever fires on
/// a loop this VM cannot leave.
pub const OPCODE_BUDGET_PER_STEP: u32 = 100_000;

/// Work_Zone is one shared global array across every entity VM in the event
/// (research/XiEvents/Event VM Functions.md getworkofs: "Work_Zone is a shared
/// global array in FFXiMain.dll for the zone to use for all events"). Every VM
/// in a scene aliases the same cell, so a setworkofs, choice selection or
/// PENDINGNUM write in one VM is visible to the master and to siblings with no
/// copy. Arc<Mutex> rather than Rc<RefCell>: the session holds a VM across an
/// await in a spawned task, so the VM must stay Send.
type SharedWorkZone = Arc<Mutex<[u32; WORK_ZONE_LEN]>>;

/// `XiEvent` runtime for a single event, simplified to the linear+jump+message
/// flow plus the per-actor request stacks a scene fans out onto (research/
/// XiEvents/Event VM Structures.md xievent_t::ReqStack). Mirrors the fields the
/// implemented opcodes touch.
pub struct EventVm {
    scene: Option<scene::Scene>,
    scene_cancelled: bool,
    scene_actions: Vec<scene::SceneAction>,
    event_data: Vec<u8>,
    references: Vec<u32>,
    work_local: [u32; WORK_LOCAL_LEN],
    /// The string view of the same WorkLocal table `work_local` reads as ints
    /// (retail stores both in one 16-byte-per-slot array; only the opcodes
    /// implemented here touch it, so the two views need not alias).
    work_local_str: [[u8; 16]; WORK_LOCAL_LEN],
    /// The s2c PENDINGSTR table (PTR_EventStrings): four 16-byte strings the
    /// server pushes before the event, read by [`OP_WINDOW`] case 1
    /// (research/XiPackets/world/server/0x005D, research/XiEvents/OpCodes/
    /// 0x00B4.md).
    pending_strings: [[u8; 16]; 4],
    work_zone: SharedWorkZone,
    exec_pointer: usize,
    jump_table: [u16; JUMP_STACK_LEN],
    jump_index: usize,
    speaker_index: u16,
    param_len: usize,
    /// `CliEventMessOpenFlag`: 0 none, 1 awaiting dismissal, 2 invalid.
    message_open: u8,
    pending_message: Option<EventMessage>,
    pending_choice: Option<EventChoice>,
    selection_made: bool,
    /// Retail's `CliEventCancelFlag`: whether ESC may cancel this event. Armed
    /// at start (plain conversations stay cancellable); `OP_CANCEL_DISARM`
    /// disarms it in the prologue of cutscenes that lock you in,
    /// `OP_CANCEL_ARM` re-arms it.
    cancel_armed: bool,
    /// Choreography cues emitted since the host last drained them — see
    /// [`Self::take_cues`].
    cues: Vec<EventCue>,
    finished: bool,
    /// Set by the last [`step`](Self::step): whether it yielded
    /// [`StepResult::Waiting`] with none of the state [`Self::park`] names,
    /// the Park::Dead half of the host's liveness check.
    last_waiting: bool,
    /// The host force-cancelled the event (the liveness stall): the next
    /// [`step`](Self::step) reports [`StepResult::Cancelled`] whatever the VM
    /// was parked on.
    force_cancelled: bool,
    /// Diagnostics: execution ran off the end of the bytecode without an
    /// END/EXECEND opcode. Retail treats this the same as END (the missing-
    /// byte read yields 0 == OP_END), so it only signals a decode or
    /// entry-point bug — see [`Self::ran_past_end`].
    ran_past_end: bool,
    /// Diagnostics: count of [`Self::eventgetcode`] operand reads that fell
    /// (fully or partly) past the end of the bytecode; each read yields 0.
    /// `Cell` because reads happen through `&self` accessors.
    oob_reads: std::cell::Cell<u32>,
    /// Retail's `ReqStack[RunPos].WaitTime`: what is left of a timed wait, and
    /// how far to step once it runs out. Armed by the opcode, drained by
    /// [`Self::tick`].
    wait: Option<Wait>,
    /// A tag sent to the server mid-event (retail's `RecPendingFlag` and its
    /// position-tag counterpart), held until [`Self::ack_server`]. While set,
    /// execution stays parked on the sending opcode's case-1 poll.
    pending_ack: Option<PendingTag>,
    /// 0xA7's response result: the EndPara the case-0 tag carried, which case
    /// 1 writes into its work slot once the server acks the tag
    /// (research/XiEvents/OpCodes/0x00A7.md).
    a7_result: u32,
    /// 0xA6's response result: the MapNum the 0x10E s2c carried, which case 2
    /// writes into its work slot; 0 when the server answered nothing and the
    /// session's grace watchdog released the hold
    /// (research/XiEvents/OpCodes/0x00A6.md).
    a6_submap_num: u32,
    /// The request this VM queued via REQEW and is still tracking, as (actor,
    /// tag): retail keeps that wait in `ReqStack[RunPos].ReqFlag` across ticks
    /// (research/XiEvents/Event VM Structures.md ReqFlag;
    /// research/XiEvents/OpCodes/0x0029.md).
    /// Set when the opcode queues its tag, cleared when that request leaves the
    /// target's stack so the re-run of the parked opcode advances instead of
    /// queueing a second child.
    req_wait: Option<(u32, u8)>,
    /// Host-armed action holds the WAIT* family parks on; see [`ActionHold`].
    action_holds: Vec<ActionHold>,
    /// Host-armed move holds a non-player MOVE case 1 parks on; see
    /// [`MoveHold`].
    move_holds: Vec<MoveHold>,
    /// The MainSpeed operand (0x32) the non-scene path arms its `OP_MOVE`
    /// `ActorMove` cue with; the scene path tracks the same value on its own
    /// `Scene` (research/XiEvents/OpCodes/0x0032.md).
    move_speed: i32,
    /// 0x31 SMOVE mode 0's goal, armed for the mode 1 that walks to it
    /// (research/XiEvents/OpCodes/0x0031.md).
    smove_goal: Option<crate::vm::scene::EventPosition>,
    /// 0x31 SMOVE's MoveTime budget in seconds, from mode 0's work slot 8;
    /// `0.0` arms no cap (research/XiEvents/OpCodes/0x0031.md).
    smove_time: f32,
    /// Set once 0x31 mode 1 has emitted its `ActorMove` cue, so a re-run of
    /// the parked opcode parks instead of emitting a second cue
    /// (research/XiEvents/OpCodes/0x0031.md).
    smove_started: bool,
    /// 0x31 mode 1's same-pass bridge: the actor its `ActorMove` cue named,
    /// held until [`Self::take_cues`] arms the move hold from it, so the
    /// parked opcode sees the move as running before the host arms it
    /// (research/XiEvents/OpCodes/0x0031.md).
    pending_move_starts: Vec<ActorLookup>,
    /// The retail entity Type byte of the actors this VM's
    /// `OP_LOADEXTSCHEDULER`/`OP_LOADEXTSCHEDULER2` opcodes name, keyed by the
    /// actor's server id and target index: the gate both motion resource
    /// readers apply before loading. An absent entry is Type 0 — retail's
    /// value when the entity has no back-ptr.
    actor_types: std::collections::HashMap<u32, u8>,
    /// Actions this VM's own `OP_LOADEVENTSCHEDULER2`/`OP_LOADEXTSCHEDULER`
    /// opcodes started within the current step, before the host has drained the
    /// cues and armed their holds. They bridge a loader to its WAIT* when both
    /// run in one pass; [`Self::take_cues`] and the start of each later
    /// [`step`](Self::step) clear them, so an action whose DAT the host cannot
    /// read falls through instead of holding forever.
    pending_action_starts: Vec<(ActorLookup, FourCc)>,
    /// Motion holds the renderer's finish report releases instead of a timer:
    /// every routine the host plays and reports (`OP_SCHEDULOR`,
    /// `OP_LOADEVENTSCHEDULER2` non-fade, `OP_LOADEXTSCHEDULER`/
    /// `OP_LOADEXTSCHEDULER2`, `OP_MAPSCHEDULOR`) parks here while it runs. See
    /// [`Self::hold_action_pending`].
    pending_action_holds: Vec<(ActorLookup, FourCc)>,
    /// Set while execution is parked on a WAIT* opcode whose hold still has
    /// frames left, so [`Self::is_waiting`] keeps the host ticking it down.
    parked_on_action_hold: bool,
    /// The same for a non-player MOVE case 1 parked on its move hold.
    parked_on_move_hold: bool,
    /// The global weather forecast table 0x72 GETWEATHER reads
    /// (research/XiEvents/OpCodes/0x0072.md). The host loads it once from the
    /// forecast DATs and shares one copy across every VM it drives; `None` in a
    /// host that never injects it, where 0x72 advances without writing.
    weather_forecast: Option<Arc<ffxi_dat::weather::WeatherForecast>>,
    /// The zone's range rects 0x82 RANGE_RECT hit-tests against
    /// (research/XiEvents/OpCodes/0x0082.md): every RID chunk of the event
    /// zone's resource DAT (ffxi-dat zone_interaction). The host loads it once
    /// per zone and shares one copy across every VM; empty when the host has no
    /// install, where 0x82 misses and jumps.
    zone_rects: Arc<Vec<ffxi_dat::zone_interaction::ZoneInteraction>>,
    /// The zone number 0xD4 case 0 opens the map on: retail's
    /// `pGlobalNowZone->ZoneNo`, injected by the host via
    /// [`Self::set_current_zone`] before driving
    /// (research/XiEvents/OpCodes/0x00D4.md).
    current_zone: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Wait {
    remaining_units: f32,
    advance: usize,
}

/// A running action the host told the VM about, so the WAIT* family can hold
/// the way retail's IsMovingAction does. Armed by the host from the
/// DAT-authored routine length when it publishes a motion cue the renderer
/// plays without a finish report (the `OP_LOADEVENTSCHEDULER2` fades); the VM
/// invents no hold of its own, so an un-armed wait falls through.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActionHold {
    actor: ActorLookup,
    key: FourCc,
    remaining_units: f32,
}

/// A running move the host told the VM about, so a non-player MOVE case 1 can
/// hold the way retail's arrival test does. Armed by the host from its own
/// distance and speed when it publishes an [`EventCue::ActorMove`]; with none
/// armed the case falls through.
#[derive(Debug, Clone, Copy, PartialEq)]
struct MoveHold {
    actor: ActorLookup,
    remaining_units: f32,
}

/// `WaitTime` decrements by `GetFrameDelay()`, which counts 1/60ths of a
/// second: research/XiEvents/OpCodes/0x005A.md scales it by `0.016666668` and
/// 0x0031.md divides it by 60.0 for per-second motion.
const WAIT_UNITS_PER_SEC: f32 = 60.0;

/// `0x6F` SLEEP authors no operand and loads this fixed duration
/// (research/XiEvents/OpCodes/0x006F.md).
const SLEEP_WAIT_UNITS: f32 = 16.0;

impl EventVm {
    /// Start `event_id` from `block` (the actor's event block), with
    /// `speaker_index` as the talking entity's target index and `params` the
    /// trigger packet's numeric parameters (`num[8]`; empty for a 0x32 trigger).
    /// `None` if the block has no such event.
    pub fn start(
        block: &EventBlock,
        event_id: u16,
        speaker_index: u16,
        params: Vec<i32>,
    ) -> Option<Self> {
        let exec_pointer = block.event_entry(event_id)?;
        Some(Self::start_at(block, exec_pointer, speaker_index, params))
    }

    pub fn driving_block(
        dat: &ffxi_dat::event_dat::EventDat,
        actor: u32,
        event_id: u16,
    ) -> Option<(&EventBlock, ffxi_dat::event_dat::EventBlockSource)> {
        let resolved = dat.block_for_event(actor, event_id)?;
        let entry = resolved.0.event_entry(event_id)?;
        if resolved.0.event_data.get(entry) != Some(&OP_END) {
            return Some(resolved);
        }
        // research/XiEvents/Event VM Functions.md InitEvent2 initializes all
        // participants. A sole non-END participant can drive an otherwise empty trigger.
        let mut driver = None;
        for block in &dat.blocks {
            let Some(entry) = block.event_entry(event_id) else {
                continue;
            };
            match block.event_data.get(entry) {
                Some(&OP_END) => {}
                Some(_) if block.event_entry_exact(event_id).is_some() && driver.is_none() => {
                    driver = Some(block)
                }
                _ => return Some(resolved),
            }
        }
        Some(
            driver
                .map(|block| {
                    (
                        block,
                        ffxi_dat::event_dat::EventBlockSource::SoleOwnerElsewhere,
                    )
                })
                .unwrap_or(resolved),
        )
    }

    /// Every block that holds a non-END exact entry for `event_id`: the owner
    /// blocks of a multi-entity event. Retail's InitEvent2 prepares each valid
    /// entity and XiEventInit starts its own block on its own ReqStack
    /// (research/XiEvents/Event VM Functions.md), so all of these run in
    /// parallel when the event starts; [`EventVm::spawn_owner`] is the
    /// event-start half of that. `event_entry_exact` is positional, so the
    /// entry is the block's own, not the master's.
    pub fn owner_blocks(
        dat: &ffxi_dat::event_dat::EventDat,
        event_id: u16,
    ) -> Vec<(&EventBlock, usize)> {
        dat.blocks
            .iter()
            .filter_map(|block| {
                let entry = block.event_entry_exact(event_id)?;
                (block.event_data.get(entry) != Some(&OP_END)).then_some((block, entry))
            })
            .collect()
    }

    fn start_at(
        block: &EventBlock,
        exec_pointer: usize,
        speaker_index: u16,
        params: Vec<i32>,
    ) -> Self {
        let mut work_zone = [0; WORK_ZONE_LEN];
        for (slot, value) in work_zone[EVENT_PARAM_WORK_BASE..][..EVENT_PARAM_COUNT]
            .iter_mut()
            .zip(&params)
        {
            *slot = *value as u32;
        }
        Self::start_at_shared(
            block,
            exec_pointer,
            speaker_index,
            params,
            Arc::new(Mutex::new(work_zone)),
        )
    }

    /// [`Self::start_at`] over the event's shared Work_Zone cell: owner and
    /// REQSET children alias the master's cell instead of snapshotting it
    /// (retail keeps one Work_Zone per event, not one per VM — see
    /// [`SharedWorkZone`]).
    fn start_at_shared(
        block: &EventBlock,
        exec_pointer: usize,
        speaker_index: u16,
        params: Vec<i32>,
        work_zone: SharedWorkZone,
    ) -> Self {
        Self {
            scene: None,
            scene_cancelled: false,
            scene_actions: Vec::new(),
            event_data: block.event_data.clone(),
            references: block.references.clone(),
            work_local: [0; WORK_LOCAL_LEN],
            work_local_str: [[0u8; 16]; WORK_LOCAL_LEN],
            pending_strings: [[0u8; 16]; 4],
            work_zone,
            exec_pointer,
            jump_table: [0; JUMP_STACK_LEN],
            jump_index: 0,
            speaker_index,
            param_len: params.len().min(EVENT_PARAM_COUNT),
            message_open: MESSAGE_OPEN_NONE,
            pending_message: None,
            pending_choice: None,
            selection_made: false,
            cancel_armed: true,
            cues: Vec::new(),
            finished: false,
            last_waiting: false,
            force_cancelled: false,
            ran_past_end: false,
            wait: None,
            pending_ack: None,
            a7_result: 0,
            a6_submap_num: 0,
            req_wait: None,
            action_holds: Vec::new(),
            move_holds: Vec::new(),
            move_speed: 0,
            smove_goal: None,
            smove_time: 0.0,
            smove_started: false,
            pending_move_starts: Vec::new(),
            actor_types: std::collections::HashMap::new(),
            pending_action_starts: Vec::new(),
            pending_action_holds: Vec::new(),
            parked_on_action_hold: false,
            parked_on_move_hold: false,
            weather_forecast: None,
            zone_rects: Arc::new(Vec::new()),
            current_zone: 0,
            oob_reads: std::cell::Cell::new(0),
        }
    }

    fn params(&self) -> Vec<i32> {
        self.work_zone
            .lock()
            .unwrap()
            .get(EVENT_PARAM_WORK_BASE..EVENT_PARAM_WORK_BASE + self.param_len)
            .expect("param_len is bounded by EVENT_PARAM_COUNT")
            .iter()
            .map(|&value| value as i32)
            .collect()
    }

    /// Clear the open-dialog flag after the player dismisses a message, so the
    /// next [`step`](Self::step) advances past MESWAIT. The frame is dropped with
    /// it: while it stays up, MESWAIT re-emits it instead of acking (retail
    /// yields every frame until dismissal). When a child request holds the open
    /// frame (`Scene::dialog_child`), the dismissal reaches that child: retail's
    /// CliEventMessOpenFlag is one global flag shared by every entity's VM
    /// (research/XiEvents/OpCodes/0x001D.md, 0x0023.md).
    pub fn dismiss_message(&mut self) {
        if let Some((actor, index)) = self.open_frame_holder() {
            self.child_mut(actor, index).dismiss_message();
            return;
        }
        self.message_open = MESSAGE_OPEN_NONE;
        self.pending_message = None;
    }

    /// A message frame the host has not dismissed yet: retail's
    /// `PTR_TalkWinFlag` for the message half of the talk window. The menu
    /// half is `pending_choice`, checked by the QUERYWAIT arms themselves.
    fn message_frame_open(&self) -> bool {
        self.pending_message.is_some() || self.message_open == MESSAGE_OPEN_AWAITING
    }

    /// Mark the open message invalid so the next MESWAIT force-cancels the
    /// event (research/XiEvents/OpCodes/0x0023.md) — the Esc-on-message path.
    pub fn cancel_message(&mut self) {
        if self.scene_waiting() {
            self.scene = None;
            self.scene_actions.clear();
            self.scene_cancelled = true;
        }
        self.message_open = MESSAGE_OPEN_INVALID;
    }

    /// Record the player's menu selection (0-based, or [`u32::MAX`] to cancel)
    /// into `Work_Zone[0]` — the slot QUERYWAIT writes and subsequent `if`
    /// opcodes branch on — so the next [`step`](Self::step) advances past
    /// QUERYWAIT. When a child request holds the open menu frame, the selection
    /// lands in that child's Work_Zone instead (see
    /// [`Self::dismiss_message`]).
    pub fn select_choice(&mut self, index: Option<u32>) {
        if let Some((actor, i)) = self.open_frame_holder() {
            self.child_mut(actor, i).select_choice(index);
            return;
        }
        self.work_zone.lock().unwrap()[0] = index.unwrap_or(CHOICE_CANCELLED);
        self.selection_made = true;
    }

    pub fn exec_pointer(&self) -> usize {
        self.exec_pointer
    }

    /// The opcode byte the VM is parked on, for the host's stall diagnostics;
    /// 0 when the pointer ran past the end of the bytecode.
    pub fn current_opcode(&self) -> u8 {
        self.event_data.get(self.exec_pointer).copied().unwrap_or(0)
    }

    /// The timed wait's remaining units (1/60 s, the [`Self::tick`] clock),
    /// 0 when no wait is held: the host's liveness check watches it move.
    pub fn wait_units_remaining(&self) -> f32 {
        self.wait.as_ref().map_or(0.0, |w| w.remaining_units)
    }

    /// Why the VM is not advancing right now, for the host's liveness check:
    /// a frame and a moving timed wait are never stale, a hold and a pending
    /// tag are stale when their release does not arrive, and a yield with
    /// nothing armed behind it is stale immediately.
    pub fn park(&self) -> Park {
        if self.force_cancelled {
            return Park::None;
        }
        if self.frame_displayed() {
            return Park::Frame;
        }
        if self.wait.is_some() {
            return Park::TimedWait;
        }
        if self.parked_on_action_hold || self.parked_on_move_hold || self.scene_waiting() {
            return Park::Hold;
        }
        if self.pending_tag().is_some() {
            return Park::ServerAck;
        }
        if self.last_waiting {
            return Park::Dead;
        }
        Park::None
    }

    /// End the event from the host side (the liveness stall): the next
    /// [`step`](Self::step) reports [`StepResult::Cancelled`] whatever the VM
    /// was parked on, and the pending tag, holds and children are dropped
    /// with it (research/XiPackets/world/client/0x005B).
    pub fn force_cancel(&mut self) {
        self.force_cancelled = true;
        self.finished = true;
        self.pending_ack = None;
        self.wait = None;
        self.action_holds.clear();
        self.move_holds.clear();
        self.pending_action_holds.clear();
        self.parked_on_action_hold = false;
        self.parked_on_move_hold = false;
        self.pending_message = None;
        self.pending_choice = None;
        self.selection_made = false;
        self.message_open = MESSAGE_OPEN_NONE;
        let mut cancel = |child: &mut EventVm| child.force_cancel();
        self.for_each_child_vm(&mut cancel);
    }

    /// Drain the [`EventCue`]s the staging opcodes emitted, in execution order.
    /// They accumulate across [`step`](Self::step) calls (one step can emit
    /// several), so the host drains after each step rather than reading a
    /// per-step return value.
    pub fn take_cues(&mut self) -> Vec<EventCue> {
        let cues = std::mem::take(&mut self.cues);
        self.pending_action_starts.clear();
        self.pending_move_starts.clear();
        if let Some(scene) = &self.scene {
            cues.into_iter()
                .map(|cue| cue.resolve_event_actor(ActorLookup(scene.actor)))
                .collect()
        } else {
            cues
        }
    }

    /// True if the program counter ran off the end of the bytecode without an
    /// END/EXECEND opcode. Retail treats this identically to END, so the event
    /// still finishes with [`StepResult::Done`]; the flag distinguishes the
    /// two for diagnostics.
    pub fn ran_past_end(&self) -> bool {
        self.ran_past_end
    }

    /// Number of `eventgetcode` operand reads that fell (fully or partly) past
    /// the end of the bytecode; each such read yielded 0.
    pub fn oob_reads(&self) -> u32 {
        self.oob_reads.get()
    }

    /// `Work_Zone[index]` as a signed value. `Work_Zone[1]` is the event-end
    /// result the client returns in the 0x05B `EndPara`
    /// (research/XiPackets/world/client/0x005B).
    pub fn work_zone(&self, index: usize) -> i32 {
        self.work_zone
            .lock()
            .unwrap()
            .get(index)
            .copied()
            .unwrap_or(0) as i32
    }

    /// Arm a hold for `key` on `actor` lasting `units` (1/60 s, the same clock
    /// as [`Self::tick`]). Replaces an existing hold for the same pair, timed
    /// or pending: last arm wins both ways. The event-entity selector resolves
    /// against the running scene's actor, the way
    /// [`EventCue::resolve_event_actor`] does, so the host passes the
    /// unresolved lookup it got from the cue.
    pub fn hold_action(&mut self, actor: ActorLookup, key: FourCc, units: f32) {
        let actor = self.resolve_hold_actor(actor);
        self.action_holds
            .retain(|h| !(h.actor == actor && h.key == key));
        self.pending_action_holds
            .retain(|(a, k)| !(a == &actor && k == &key));
        self.action_holds.push(ActionHold {
            actor,
            key,
            remaining_units: units,
        });
    }

    /// Arm a motion hold with no length of its own: the host plays the routine
    /// in the renderer and releases it via [`Self::release_action_hold`] when
    /// the renderer reports the routine finished, instead of by a timer.
    /// Replaces any timed or pending hold for the same pair, the way
    /// [`Self::hold_action`] does.
    pub fn hold_action_pending(&mut self, actor: ActorLookup, key: FourCc) {
        let actor = self.resolve_hold_actor(actor);
        self.action_holds
            .retain(|h| !(h.actor == actor && h.key == key));
        self.pending_action_holds
            .retain(|(a, k)| !(a == &actor && k == &key));
        self.pending_action_holds.push((actor, key));
    }

    /// Release the motion hold the renderer's finish report names.
    /// No-op when nothing is pending for the pair, so a report for a routine
    /// outside the pending set (stopped, superseded, event ended) is safe.
    pub fn release_action_hold(&mut self, actor: ActorLookup, key: FourCc) {
        let actor = self.resolve_hold_actor(actor);
        self.pending_action_holds
            .retain(|(a, k)| !(a == &actor && k == &key));
    }

    /// Replace the entity Type table the `OP_LOADEXTSCHEDULER`/
    /// `OP_LOADEXTSCHEDULER2` gate reads (see [`Self::actor_type`]). The session
    /// calls this with its current map before every drive, so a late entity
    /// update lands before the next step.
    pub fn set_actor_types(&mut self, types: &std::collections::HashMap<u32, u8>) {
        self.actor_types = types.clone();
    }

    /// Install the global weather forecast table 0x72 GETWEATHER reads
    /// (research/XiEvents/OpCodes/0x0072.md). The host loads it once from the
    /// install's forecast DATs and shares the same `Arc` across every VM, so the
    /// table is read, not copied, per event. No-op on a second install.
    pub fn set_weather_forecast(
        &mut self,
        forecast: std::sync::Arc<ffxi_dat::weather::WeatherForecast>,
    ) {
        self.weather_forecast = Some(forecast.clone());
        let mut land = |child: &mut EventVm| child.set_weather_forecast(forecast.clone());
        self.for_each_child_vm(&mut land);
    }

    /// Install the zone's range rects 0x82 RANGE_RECT hit-tests against
    /// (research/XiEvents/OpCodes/0x0082.md). The host loads the event zone's
    /// RID table once and shares the same `Arc` across every VM it drives, so
    /// the table is read, not copied, per event. An empty install leaves the
    /// default empty table, where 0x82 always misses.
    pub fn set_zone_rects(
        &mut self,
        rects: std::sync::Arc<Vec<ffxi_dat::zone_interaction::ZoneInteraction>>,
    ) {
        self.zone_rects = rects.clone();
        let mut land = |child: &mut EventVm| child.set_zone_rects(rects.clone());
        self.for_each_child_vm(&mut land);
    }

    /// Install the zone number 0xD4 case 0 opens the map on
    /// (research/XiEvents/OpCodes/0x00D4.md). The host injects the event zone
    /// before driving; a host that never does leaves the default 0, where
    /// case 0 opens the map on zone 0.
    pub fn set_current_zone(&mut self, zone: i32) {
        self.current_zone = zone;
        let mut land = |child: &mut EventVm| child.set_current_zone(zone);
        self.for_each_child_vm(&mut land);
    }

    /// 0x82's hit test: whether the event entity's tracked position falls inside
    /// the zone range rect named by `rect_id`. `false` when no scene tracks a
    /// position (retail's null-entity early return) or no rect matches
    /// (research/XiEvents/OpCodes/0x0082.md).
    fn range_rect_hit(&self, rect_id: u32) -> bool {
        let Some(pos) = self.event_entity_rid_position() else {
            return false;
        };
        self.zone_rects
            .iter()
            .any(|r| r.rect_id() == rect_id && r.contains(pos))
    }

    /// Arm the 0x24-style choice the 0xD4 cases 0/2 open. The embedded 0x24
    /// helper is entered one byte past the opcode start, so its message and
    /// default-cursor selectors sit at +2 and +4 here, not +1 and +3 as on a
    /// standalone 0x24 (research/XiEvents/OpCodes/0x00D4.md, 0x0024.md).
    fn arm_map_query_choice(&mut self) {
        self.pending_choice = Some(EventChoice {
            message_id: self.getworkofs(2, 0) as u32,
            speaker_index: self.speaker_index,
            default_index: self.getworkofs(4, 0) as u32,
            params: self.params(),
        });
        self.selection_made = false;
    }

    /// The retail entity Type byte the `OP_LOADEXTSCHEDULER`/
    /// `OP_LOADEXTSCHEDULER2` gate applies to `actor`: the former loads only
    /// for Type {1,2,7,8}, the latter only for {0,1,6}. Resolution stays in
    /// the hold-actor space: the local player is CHAR_PC (Type 0), the
    /// event-entity selector and the default-handler fallback resolve to the
    /// running scene's actor (falling back to the speaker's target index when
    /// no scene is attached), and a literal server id resolves to its target
    /// index. Anything else — party/alliance selectors, an entity the host
    /// has not published — is Type 0, retail's value when the entity has no
    /// back-ptr.
    fn actor_type(&self, actor: ActorLookup) -> u8 {
        if actor.is_local_player() {
            return 0;
        }
        if actor.is_event_entity() {
            return self
                .scene
                .as_ref()
                .and_then(|s| self.actor_types.get(&s.actor).copied())
                .or_else(|| self.actor_types.get(&(self.speaker_index as u32)).copied())
                .unwrap_or(0);
        }
        actor
            .target_index()
            .and_then(|t| self.actor_types.get(&(t as u32)).copied())
            .unwrap_or(0)
    }

    /// Arm a move hold on `actor` lasting `units` (1/60 s, the same clock as
    /// [`Self::tick`]). Replaces an existing hold for the same actor. The host
    /// computes units from its own entity distance and speed when it publishes
    /// an [`EventCue::ActorMove`]; the VM measures no move itself.
    pub fn hold_move(&mut self, actor: ActorLookup, units: f32) {
        let actor = self.resolve_hold_actor(actor);
        self.move_holds.retain(|h| h.actor != actor);
        self.move_holds.push(MoveHold {
            actor,
            remaining_units: units,
        });
    }

    /// True while a host-armed move hold for `actor` still has frames left, or
    /// this VM's own 0x31 cue named `actor` in the current batch, before the
    /// host armed the hold from it (research/XiEvents/OpCodes/0x0031.md).
    fn move_running(&self, actor: ActorLookup) -> bool {
        let actor = self.resolve_hold_actor(actor);
        self.move_holds
            .iter()
            .any(|h| h.actor == actor && h.remaining_units > 0.0)
            || self.pending_move_starts.contains(&actor)
    }

    /// True while a host-armed hold for `(actor, key)` still has frames left,
    /// a motion hold is still pending its renderer finish report, or the
    /// action was started by this VM's own loader in the current batch, before
    /// the host armed its hold from the cue.
    fn action_running(&self, actor: ActorLookup, key: FourCc) -> bool {
        let actor = self.resolve_hold_actor(actor);
        if self
            .action_holds
            .iter()
            .any(|h| h.actor == actor && h.key == key && h.remaining_units > 0.0)
        {
            return true;
        }
        if self
            .pending_action_holds
            .iter()
            .any(|(a, k)| *a == actor && *k == key)
        {
            return true;
        }
        self.pending_action_starts
            .iter()
            .any(|(a, k)| *a == actor && *k == key)
    }

    /// Resolve a hold's actor the way [`Self::take_cues`] resolves cue actors:
    /// the event-entity selector maps to the running scene's actor, everything
    /// else is identity.
    fn resolve_hold_actor(&self, actor: ActorLookup) -> ActorLookup {
        if actor.is_event_entity() {
            if let Some(scene) = &self.scene {
                return ActorLookup(scene.actor);
            }
        }
        actor
    }

    /// `FUNC_SearchUniqueID` (research/XiEvents/OpCodes/0x0044.md): non-zero
    /// when an actor with that server id is in the pool. The host publishes
    /// event participants under their target index (kuluu-session's
    /// note_entity_type), so a published target index is the modelled "in the
    /// pool"; a reserved selector or an unpublished id is not.
    fn entity_in_pool(&self, server_id: u32) -> bool {
        ActorLookup(server_id)
            .target_index()
            .is_some_and(|t| self.actor_types.contains_key(&(t as u32)))
    }

    /// Whether retail's `GetActorIndex` would succeed for `actor` in this
    /// model (research/XiEvents/Event VM Functions.md GetActorIndex): the
    /// reserved selectors the host always stands in for, or a literal server
    /// id the host has published.
    fn actor_resolved(&self, actor: ActorLookup) -> bool {
        actor.is_local_player()
            || actor == ActorLookup::EVENT_ENTITY
            || self.entity_in_pool(actor.0)
    }

    /// True while any host-armed or this-pass started action holds the event
    /// entity: retail's `AnimationPlay` is a per-entity "any animation" flag
    /// (research/XiEvents/OpCodes/0x009B.md), not a routine slot.
    fn entity_animation_running(&self) -> bool {
        let actor = self.resolve_hold_actor(ActorLookup::EVENT_ENTITY);
        self.action_holds
            .iter()
            .any(|h| h.actor == actor && h.remaining_units > 0.0)
            || self.pending_action_holds.iter().any(|(a, _)| *a == actor)
            || self.pending_action_starts.iter().any(|(a, _)| *a == actor)
    }

    fn arm_wait(&mut self, units: f32, advance: usize) -> StepResult {
        self.wait = Some(Wait {
            remaining_units: units,
            advance,
        });
        StepResult::Waiting
    }

    /// Run the host's clock into the wait. Retail decrements once per frame and
    /// steps the pointer on the frame the timer goes negative, so a zero-length
    /// wait still costs a tick — which is what keeps a scene's cues from all
    /// landing together.
    pub fn tick(&mut self, dt_secs: f32) {
        self.tick_scene(dt_secs);
        self.tick_stacks(dt_secs);
        let hold_dt = dt_secs * WAIT_UNITS_PER_SEC;
        for hold in &mut self.action_holds {
            hold.remaining_units -= hold_dt;
        }
        self.action_holds.retain(|h| h.remaining_units > 0.0);
        for hold in &mut self.move_holds {
            hold.remaining_units -= hold_dt;
        }
        self.move_holds.retain(|h| h.remaining_units > 0.0);
        let Some(wait) = self.wait.as_mut() else {
            return;
        };
        wait.remaining_units -= dt_secs * WAIT_UNITS_PER_SEC;
        if wait.remaining_units < 0.0 {
            self.exec_pointer += wait.advance;
            self.wait = None;
        }
    }

    pub fn is_waiting(&self) -> bool {
        self.wait.is_some()
            || self.scene_waiting()
            || self.parked_on_action_hold
            || self.parked_on_move_hold
    }

    /// Whether ESC may cancel this event right now (retail's `CliEventCancelFlag`).
    pub fn cancel_armed(&self) -> bool {
        self.cancel_armed
    }

    /// True while a dialog frame is displayed and unanswered: a message parked
    /// on its MESWAIT with retail's CliEventMessOpenFlag up
    /// (research/XiEvents/OpCodes/0x0023.md), or a menu parked on its QUERYWAIT
    /// (0x0024.md). The host shows the box exactly for this span, so both kinds
    /// must count — an answered menu closes its frame just as a dismissed
    /// message does. A child request holding the open frame counts too (retail's
    /// one global flag is shared by every entity's VM).
    pub fn frame_displayed(&self) -> bool {
        self.open_frame_holder().is_some()
            || self.message_open == MESSAGE_OPEN_AWAITING
            || self.pending_choice.is_some()
    }

    /// The server acknowledged the pending tag (s2c PENDINGNUM/PENDINGSTR):
    /// clear it and step past the case-1 poll opcode execution is parked on.
    /// Retail's next tick sees `RecPendingFlag` cleared, advances +2, and
    /// yields; doing that advance here instead of re-running the poll is
    /// behaviorally identical one frame earlier (research/XiEvents/OpCodes/
    /// 0x0043.md, 0x0047.md). No-op if nothing is pending. Retail's
    /// `RecPendingFlag`/`RecPendingXZYFlag` are single globals shared by every
    /// entity VM in the event, so the release reaches every parked VM, not
    /// just the one that sent the tag.
    pub fn ack_server(&mut self) {
        if self.pending_ack.take().is_some() {
            self.exec_pointer += 2;
        }
        let mut release = |child: &mut EventVm| child.ack_server();
        self.for_each_child_vm(&mut release);
    }

    /// s2c PENDINGNUM's num[8] copied into Work_Zone starting at index 2
    /// (research/XiPackets/world/server/0x005C GP_SERV_PENDINGNUM). The event
    /// system reads these slots as its loop conditions, so the write lands
    /// before the next step even while a tag is held. Work_Zone is the
    /// event's shared cell ([`SharedWorkZone`]), so the write is visible to
    /// every VM in the scene at once.
    pub fn apply_pending_num(&mut self, num: &[i32; 8]) {
        for (slot, value) in num.iter().enumerate() {
            if let Some(cell) = self
                .work_zone
                .lock()
                .unwrap()
                .get_mut(PENDING_NUM_WORK_ZONE_BASE + slot)
            {
                *cell = *value as u32;
            }
        }
    }

    /// s2c 0x005D PENDINGSTR's four 16-byte strings copied into the event
    /// string table (PTR_EventStrings); 0xB4 case 1 reads them by the work
    /// operand each instruction carries (research/XiPackets/world/server/
    /// 0x005D, research/XiEvents/OpCodes/0x00B4.md). Lands before the next
    /// step even while a tag is held, like [`Self::apply_pending_num`];
    /// PTR_EventStrings is one global per event, so the table lands in every
    /// VM's copy at once.
    pub fn apply_pending_str(&mut self, strings: &[[u8; 16]; 4]) {
        self.pending_strings = *strings;
        let mut land = |child: &mut EventVm| child.apply_pending_str(strings);
        self.for_each_child_vm(&mut land);
    }

    /// s2c 0x10E REQSUBMAPNUM's MapNum into the 0xA6 result slot case 2 reads;
    /// lands before the next step even while the SubMapNum tag is held, like
    /// [`Self::apply_pending_num`]. The tag is one global per event, so the
    /// value lands in every VM's copy at once
    /// (research/XiEvents/OpCodes/0x00A6.md).
    pub fn set_submap_num(&mut self, num: u32) {
        self.a6_submap_num = num;
        let mut land = |child: &mut EventVm| child.set_submap_num(num);
        self.for_each_child_vm(&mut land);
    }

    /// The tag held on its case-1 poll, if any: this VM's own, else the first
    /// one held by a child (the tag is one global per event in retail, so an
    /// owner child can be the holder the host must gate on).
    pub fn pending_tag(&self) -> Option<&PendingTag> {
        self.pending_ack
            .as_ref()
            .or_else(|| self.child_pending_tag())
    }

    /// The result a finished master reports: while actor request stacks still
    /// hold work, retail keeps ticking those actors and the event only ends
    /// when they all drain.
    fn finish_result(&self) -> StepResult {
        if self.scene_waiting() {
            StepResult::Waiting
        } else {
            StepResult::Done
        }
    }

    /// Run opcodes until the VM yields (one `EventIdle` tick).
    pub fn step(&mut self) -> StepResult {
        let result = self.step_inner();
        self.last_waiting = matches!(result, StepResult::Waiting);
        result
    }

    /// The yield loop [`step`](Self::step) runs.
    fn step_inner(&mut self) -> StepResult {
        if self.force_cancelled {
            return StepResult::Cancelled;
        }
        if self.scene_cancelled {
            return StepResult::Cancelled;
        }
        // A new EventIdle tick: retail re-queries IsMovingAction from live
        // render state, so the same-pass bridge (this VM's own loader cues not
        // yet armed by the host) spans only the pass that emitted them
        // (research/XiEvents/OpCodes/0x0053.md).
        self.pending_action_starts.clear();
        // The actor request stacks run on this frame before the master's own
        // opcodes, the way retail's RunPos ticks every actor each EventIdle. A
        // child that parked on a dialog frame surfaces it to the host instead of
        // running further: retail yields with its global open flag up until the
        // player answers (research/XiEvents/OpCodes/0x0023.md).
        if let Some(frame) = self.step_stacks() {
            return frame;
        }
        if self.finished {
            return self.finish_result();
        }
        // A displayed frame holds execution at its MESWAIT until dismissal:
        // retail yields every tick with the open flag up and runs nothing else,
        // so re-stepping here must not reach the opcode again
        // (research/XiEvents/OpCodes/0x0023.md).
        if self.message_open == MESSAGE_OPEN_AWAITING && self.pending_message.is_some() {
            return StepResult::Waiting;
        }
        if self.wait.is_some() {
            return StepResult::Waiting;
        }
        if let Some(tag) = &self.pending_ack {
            return StepResult::AwaitServerAck(tag.clone());
        }
        self.resume_scene();
        let mut budget = OPCODE_BUDGET_PER_STEP;
        loop {
            let Some(&op) = self.event_data.get(self.exec_pointer) else {
                // Retail reads 0 (== OP_END) here, so ending is faithful; flag
                // it because a well-formed event always terminates via
                // END/EXECEND and this usually means a bad entry point or a
                // decode bug (kuluu-zkuf).
                self.finished = true;
                self.ran_past_end = true;
                tracing::debug!(
                    exec_pointer = self.exec_pointer,
                    bytecode_len = self.event_data.len(),
                    "event VM ran past end of bytecode without END opcode"
                );
                return self.finish_result();
            };
            // Retail runs the program each frame until an opcode sets RetFlag,
            // and authored events reach one. Ours can miss it, because the
            // opcodes it steps over blind include the ones that would have
            // moved a loop's condition along; without a budget that is a hung
            // client rather than a dropped scene
            // (research/XiEvents/Event VM Functions.md EventIdle).
            budget -= 1;
            if budget == 0 {
                self.finished = true;
                tracing::warn!(
                    exec_pointer = self.exec_pointer,
                    op = format!("0x{op:02X}"),
                    "event VM exceeded its opcode budget; the script is looping \
                     on state this VM does not model"
                );
                return StepResult::Spun(op);
            }
            if self.handles_scene_opcode(op) {
                if let Some(result) = self.scene_opcode(op) {
                    return result;
                }
                continue;
            }
            match op {
                OP_END => {
                    self.finished = true;
                    return self.finish_result();
                }
                // 0x21 sets EventExecEnd, which stops XiEvent::EventIdle from
                // running the program again — the event is over
                // (research/XiEvents/OpCodes/0x0021.md).
                OP_EXECEND => {
                    self.finished = true;
                    return self.finish_result();
                }
                // 0x22 sets/clears the event-hide render flag on the event's own entity
                // (EntityTargetIndex[1], no target operand) — research/XiEvents/OpCodes/0x0022.md.
                // It rides the same cue as 0x4E; retail stops consulting the flag when the
                // event ends, so release_cutscene_actors owns the unhide.
                OP_EVENT_HIDE_SELF => {
                    self.cues.push(EventCue::ActorHide {
                        target: ActorLookup::EVENT_ENTITY,
                        hide: self.byte_at(EVENTHIDE_FLAG_OFS) & EVENTHIDE_FLAG_MASK != 0,
                    });
                    self.advance(op);
                }
                // The handler writes the operand's high byte with 0x20 forced
                // into CliEventModeLocal's lower word; retail's Ailevia tour
                // stores 0x2003, which applies as 0x20
                // (research/XiEvents/OpCodes/0x0038.md).
                OP_LOCAL_MODE => {
                    let val = self.getworkofs(1, 0);
                    self.cues.push(EventCue::LocalMode {
                        mode: ((val >> 8) & LOCAL_MODE_BYTE_MASK) as u16 | LOCAL_MODE_FORCED_BIT,
                    });
                    self.advance(op);
                }
                OP_GOTO => self.exec_pointer = self.eventgetcode(1) as usize,
                OP_IF => self.op_if(),
                OP_GET_STORE => {
                    let val = self.getworkofs(3, 0);
                    self.setworkofs(1, val, 0);
                    self.exec_pointer += 5;
                }
                // The work-slot arithmetic family (XiEvents OpCodes/0x0005-0x0019).
                // Pure integer math on the work stores, no host state — but a
                // counter these advance is what an authored loop tests, so
                // skipping them by width is what turns a `for` into a spin
                // (kuluu-cjct). Wrapping throughout: retail is C++ `int` on x86,
                // and a panicking client is worse than a wrong scene.
                OP_SET_ONE => self.store_unary(1, 3),
                OP_SET_ZERO => self.store_unary(0, 3),
                OP_INC => {
                    let val = self.getworkofs(1, 0);
                    self.store_unary(val.wrapping_add(1), 3);
                }
                OP_DEC => {
                    let val = self.getworkofs(1, 0);
                    self.store_unary(val.wrapping_sub(1), 3);
                }
                OP_ADD => self.store_binary(|a, b| a.wrapping_add(b)),
                OP_SUB => self.store_binary(|a, b| a.wrapping_sub(b)),
                OP_MUL => self.store_binary(|a, b| a.wrapping_mul(b)),
                // Retail guards on both operands, so a zero numerator yields 0
                // rather than dividing
                // (research/XiEvents/OpCodes/0x0015.md); wrapping_div
                // additionally spares us the i32::MIN / -1 panic where x86
                // idiv would trap.
                OP_DIV => self.store_binary(|a, b| {
                    if a != 0 && b != 0 {
                        a.wrapping_div(b)
                    } else {
                        0
                    }
                }),
                OP_AND => self.store_binary(|a, b| a & b),
                OP_OR => self.store_binary(|a, b| a | b),
                OP_XOR => self.store_binary(|a, b| a ^ b),
                // `wrapping_shl`/`shr` mask the shift to 5 bits, which is what
                // the x86 shift retail compiles to already does
                // (research/XiEvents/OpCodes/0x0010.md, 0x0011.md).
                OP_SHL => self.store_binary(|a, b| a.wrapping_shl(b as u32)),
                OP_SHR => self.store_binary(|a, b| a.wrapping_shr(b as u32)),
                OP_BIT_SET => self.store_binary(|a, b| 1i32.wrapping_shl(b as u32) | a),
                OP_BIT_CLEAR => self.store_binary(|a, b| !1i32.wrapping_shl(b as u32) & a),
                OP_SWAP => {
                    let v1 = self.getworkofs(1, 0);
                    let v2 = self.getworkofs(3, 0);
                    self.setworkofs(1, v2, 0);
                    self.setworkofs(3, v1, 0);
                    self.exec_pointer += 5;
                }
                // Sets one bit in a work-slot bit array: operand 3 is the flat
                // bit index, split into slot and bit the same way as BITTEST, and
                // operand 5 bounds the array (research/XiEvents/OpCodes/0x003C.md).
                OP_BITARRAY_SET => {
                    let bit = self.getworkofs(3, 0);
                    let slot = bit >> BIT_TEST_WORD_SHIFT;
                    if slot < self.getworkofs(5, 0) {
                        let prev = self.getworkofs(1, slot);
                        self.setworkofs(
                            1,
                            1i32.wrapping_shl((bit & BIT_TEST_BIT_MASK) as u32) | prev,
                            slot,
                        );
                    }
                    self.exec_pointer += 7;
                }
                OP_SETBITWORK => {
                    self.op_bitwork(true);
                    self.exec_pointer += 9;
                }
                OP_GETBITWORK => {
                    self.op_bitwork(false);
                    self.exec_pointer += 9;
                }
                // Retail yields here only while the event entity is mid-turn,
                // which is render state we do not model, so only its other path
                // is reachable (research/XiEvents/OpCodes/0x0070.md).
                OP_TURNWAIT => self.exec_pointer += 1,
                OP_SLEEP => return self.arm_wait(SLEEP_WAIT_UNITS, 1),
                OP_WAIT => {
                    let units = self.getworkofs(1, 0) as f32;
                    return self.arm_wait(units, 3);
                }
                // Case 0 sends the pending EVENTEND tag (EndPara = Work_Zone[1])
                // and sets RecPendingFlag; execution continues into the following
                // case-1 poll, which yields until the server acks. Case 1 with no
                // outstanding tag skips past (+2); retail spins on any other case
                // byte, so authored data cannot hold one (research/XiEvents/OpCodes/0x0043.md).
                OP_SENDTAG => match self.byte_at(1) {
                    0 => {
                        self.pending_ack = Some(PendingTag::SendTag {
                            end_para: self.work_zone(1) as u32,
                        });
                        self.exec_pointer += 2;
                    }
                    1 => match &self.pending_ack {
                        Some(tag) => return StepResult::AwaitServerAck(tag.clone()),
                        None => self.exec_pointer += 2,
                    },
                    _ => self.exec_pointer += 2,
                },
                // Case 0 sends the position tag: c2s EVENTENDXZY with x/y/z
                // scaled from work-slot raw values and the heading byte on LSB's
                // 0..=255 rotation scale (research/XiEvents/OpCodes/0x0047.md;
                // vendor/server/src/common/utils.cpp radianToRotation).
                // Execution continues into the following case-1 poll, which
                // yields until the server acks; case 1 with no outstanding tag
                // skips past.
                OP_EVENTPOSSET => match self.byte_at(1) {
                    0 => {
                        // Retail evaluates left-to-right with its own literals
                        // (research/XiEvents/OpCodes/0x0047.md); the fold order
                        // is load-bearing at the last ulp. 6.283 is retail's
                        // authored literal, not a stand-in for TAU, so it stays exact.
                        #[allow(clippy::approx_constant)]
                        let radians = self.getworkofs(8, 0) as f32 * 6.283 * 0.00024414062;
                        self.pending_ack = Some(PendingTag::SendXzy {
                            x: self.getworkofs(2, 0) as f32 * XZY_COORD_SCALE,
                            y: self.getworkofs(4, 0) as f32 * XZY_COORD_SCALE,
                            z: self.getworkofs(6, 0) as f32 * XZY_COORD_SCALE,
                            dir: ((radians / (2.0 * std::f32::consts::PI)) * 256.0) as u8,
                            end_para: self.work_zone(1) as u32,
                        });
                        self.exec_pointer += 10;
                    }
                    1 => match &self.pending_ack {
                        Some(tag) => return StepResult::AwaitServerAck(tag.clone()),
                        None => self.exec_pointer += 2,
                    },
                    _ => self.exec_pointer += 2,
                },
                // 0xA7: waits on the server's response to the client's request.
                // Case 0 arms the await bit and sends the pending tag
                // (EndPara = Work_Zone[1]), holding until the server acks; case
                // 1 writes the result the ack carried into the work slot its
                // +2 operand selects. The result is the EndPara the tag carried
                // — the value the session's ack path already knows — so it is
                // captured when the tag is armed
                // (research/XiEvents/OpCodes/0x00A7.md). Retail spins on any
                // other case byte, so authored data cannot hold one.
                OP_A7_WAIT => match self.byte_at(1) {
                    0 => {
                        if self.pending_ack.is_none() {
                            self.a7_result = self.work_zone(1) as u32;
                            self.pending_ack = Some(PendingTag::SendTag {
                                end_para: self.a7_result,
                            });
                        }
                        let tag = self.pending_ack.clone().expect("armed above");
                        return StepResult::AwaitServerAck(tag);
                    }
                    1 => {
                        self.setworkofs(2, self.a7_result as i32, 0);
                        self.exec_pointer += 4;
                    }
                    _ => self.exec_pointer += 2,
                },
                // 0xA6: case 0 sends the header-only 0x0EB REQSUBMAPNUM and
                // holds on the 0x10E answer; case 1 yields until it lands; case
                // 2 writes the answered MapNum into the work slot its +2 operand
                // selects. The server answers 0 when the char is npc-locked and
                // nothing otherwise, so an unanswered request is released by the
                // session's grace watchdog, which lands case 2 on 0
                // (research/XiEvents/OpCodes/0x00A6.md). Retail spins on any
                // other case byte, so authored data cannot hold one.
                OP_A6_SUBMAP => match self.byte_at(1) {
                    0 => {
                        if self.pending_ack.is_none() {
                            self.pending_ack = Some(PendingTag::SubMapNum);
                        }
                        let tag = self.pending_ack.clone().expect("armed above");
                        return StepResult::AwaitServerAck(tag);
                    }
                    1 => {
                        self.exec_pointer += 2;
                    }
                    2 => {
                        self.setworkofs(2, self.a6_submap_num as i32, 0);
                        self.exec_pointer += 4;
                    }
                    _ => self.exec_pointer += 2,
                },
                // 0xB2: mode 0 counts down WaitTime — the u16 at +1, which
                // spans the mode byte and the first operand byte, so authors
                // steer it to work_zone[0] with a 0x10 first byte — by frame
                // delay and steps +4 on expiry. Mode 1 requests delivery mode:
                // it arms the 0x04D PBX open request and holds until the 0x04B
                // DELI_OPEN/POST_OPEN ack, then +2. Retail spins on any other
                // mode byte, so authored data cannot hold one
                // (research/XiEvents/OpCodes/0x00B2.md).
                OP_B2_DELIVERY => match self.byte_at(1) {
                    0 => return self.arm_wait(self.getworkofs(1, 0) as f32, 4),
                    1 => {
                        if self.pending_ack.is_none() {
                            self.pending_ack = Some(PendingTag::DeliveryOpen);
                        }
                        let tag = self.pending_ack.clone().expect("armed above");
                        return StepResult::AwaitServerAck(tag);
                    }
                    _ => self.exec_pointer += 2,
                },
                // 0x87/0x88: the world pass. The send cases (0 and 2) arm
                // the 0x01B FRIENDPASS with the case's `Para` and hold on the
                // 0x059 answer; case 1 yields until it lands. The server
                // answers a random pass number for the confirm cases and 0
                // for the begin cases; the number has no kuluu display, so
                // the receipt only clears the await
                // (vendor/server/src/map/packets/c2s/0x01b_friendpass.cpp).
                // Retail spins on any other case byte, so authored data
                // cannot hold one.
                OP_FRIENDPASS_87 | OP_FRIENDPASS_88 => {
                    let sub = self.byte_at(1);
                    let para = match (op, sub) {
                        (OP_FRIENDPASS_87, 0) => 0,
                        (OP_FRIENDPASS_88, 0) => 1,
                        (OP_FRIENDPASS_87, 2) => 2,
                        (OP_FRIENDPASS_88, 2) => 3,
                        _ => 0,
                    };
                    match sub {
                        0 | 2 => {
                            if self.pending_ack.is_none() {
                                self.pending_ack = Some(PendingTag::FriendPass { para });
                            }
                            let tag = self.pending_ack.clone().expect("armed above");
                            return StepResult::AwaitServerAck(tag);
                        }
                        _ => self.exec_pointer += 2,
                    }
                }
                // 0x8C: the crafting support. The send cases (0/2/3/4) fill
                // the 0x058 RECIPE with their Mode and the work-slot fields
                // the retail pseudo code names, arm the await on the 0x031
                // answer, and hold; case 1 yields until it lands. Mode 4 has
                // no LSB handler, so its answer is the grace watchdog; retail
                // case 5 sends mode 5 without setting RecRecipeFlag at all
                // and LSB implements no mode 5, so it advances its width
                // without a send (research/XiEvents/OpCodes/0x008C.md;
                // vendor/server/src/map/packets/c2s/0x058_recipe.cpp). Retail
                // spins on any other case byte, so authored data cannot hold
                // one.
                OP_RECIPE => match self.byte_at(1) {
                    0 | 2 | 3 | 4 => {
                        let recipe = match self.byte_at(1) {
                            0 => PendingTag::Recipe {
                                mode: 1,
                                skill: self.getworkofs(2, 0) as u16,
                                level: self.getworkofs(4, 0) as u16,
                                param0: self.getworkofs(6, 0) as u16,
                                param1: 0,
                                param2: 0,
                                param3: 0,
                                param4: 0,
                            },
                            2 => PendingTag::Recipe {
                                mode: 2,
                                skill: self.getworkofs(2, 0) as u16,
                                level: self.getworkofs(4, 0) as u16,
                                param0: 0,
                                param1: self.getworkofs(8, 0) as u16,
                                param2: self.getworkofs(10, 0) as u16,
                                param3: 0,
                                param4: self.getworkofs(6, 0) as u16,
                            },
                            _ => PendingTag::Recipe {
                                mode: u16::from(self.byte_at(1) == 4) + 3,
                                skill: self.getworkofs(2, 0) as u16,
                                level: self.getworkofs(4, 0) as u16,
                                param0: 0,
                                param1: 0,
                                param2: 0,
                                param3: self.getworkofs(8, 0) as u16,
                                param4: self.getworkofs(6, 0) as u16,
                            },
                        };
                        if self.pending_ack.is_none() {
                            self.pending_ack = Some(recipe);
                        }
                        let tag = self.pending_ack.clone().expect("armed above");
                        return StepResult::AwaitServerAck(tag);
                    }
                    1 => self.exec_pointer += 2,
                    5 => self.exec_pointer += 14,
                    _ => self.exec_pointer += 2,
                },
                OP_JUMP => {
                    if self.jump_index == JUMP_STACK_LEN {
                        self.finished = true;
                        return StepResult::Done;
                    }
                    self.jump_table[self.jump_index] = (self.exec_pointer + 3) as u16;
                    self.jump_index += 1;
                    self.exec_pointer = self.eventgetcode(1) as usize;
                }
                OP_RETURN => {
                    if self.jump_index == 0 {
                        self.finished = true;
                        return self.finish_result();
                    }
                    self.jump_index -= 1;
                    self.exec_pointer = self.jump_table[self.jump_index] as usize;
                }
                OP_MESSAGE => {
                    let message_id = self.getworkofs(MESSAGE_ID_OFS, 0) as u32;
                    self.open_message(message_id, Some(self.speaker_index));
                    self.advance(op);
                    return self.emit_open_message();
                }
                OP_MESSAGE_ACTOR => {
                    let speaker = self.actor_index(self.eventgetcode2(ACTOR_LOOKUP_OFS));
                    let message_id = self.getworkofs(ACTOR_MESSAGE_ID_OFS, 0) as u32;
                    self.open_message(message_id, Some(speaker));
                    self.advance(op);
                    return self.emit_open_message();
                }
                OP_MESSAGE_UNNAMED => {
                    let message_id = self.getworkofs(MESSAGE_ID_OFS, 0) as u32;
                    self.open_message(message_id, None);
                    self.advance(op);
                    return self.emit_open_message();
                }
                // 0x49 resolves an actor into MESCASNAMEINDEX/MESTARNAMEINDEX but
                // prints through the nameless EventMessDecodePut, so the line
                // carries no speaker (research/XiEvents/OpCodes/0x0049.md).
                OP_MESSAGE_UNNAMED_ACTOR => {
                    let message_id = self.getworkofs(ACTOR_MESSAGE_ID_OFS, 0) as u32;
                    self.open_message(message_id, None);
                    self.advance(op);
                    return self.emit_open_message();
                }
                OP_MESSAGE_ACTOR_PAIR => {
                    // Retail returns from the handler without advancing when this
                    // byte is set — a hang unless it is always 0, so treat a set
                    // byte as bytecode we cannot run
                    // (research/XiEvents/OpCodes/0x00B0.md).
                    if self.byte_at(ACTOR_PAIR_STALL_FLAG_OFS) != 0 {
                        return StepResult::Unimplemented(op);
                    }
                    let speaker = self.actor_index(self.eventgetcode2(ACTOR_PAIR_SPEAKER_OFS));
                    let message_id = self.getworkofs(ACTOR_PAIR_MESSAGE_ID_OFS, 0) as u32;
                    self.open_message(message_id, Some(speaker));
                    self.advance(op);
                    return self.emit_open_message();
                }
                OP_MESWAIT => match self.message_open {
                    MESSAGE_OPEN_NONE => self.exec_pointer += 1,
                    MESSAGE_OPEN_INVALID => {
                        self.finished = true;
                        return StepResult::Cancelled;
                    }
                    _ => {
                        // The frame is up (message_open AWAITING): emit it and
                        // keep it pending so re-steps park at the top of
                        // [`Self::step`] until dismissal; [`Self::dismiss_message`]
                        // clears both (research/XiEvents/OpCodes/0x0023.md).
                        return match self.pending_message.clone() {
                            Some(msg) => StepResult::AwaitMessage(msg),
                            None => StepResult::AwaitMessageAck,
                        };
                    }
                },
                OP_QUERY => {
                    self.pending_choice = Some(EventChoice {
                        message_id: self.getworkofs(1, 0) as u32,
                        speaker_index: self.speaker_index,
                        default_index: self.getworkofs(3, 0) as u32,
                        params: self.params(),
                    });
                    self.selection_made = false;
                    self.exec_pointer += 7;
                }
                // With no menu armed and no message frame open, retail's
                // `!PTR_TalkWinFlag` path steps past the opcode and yields;
                // the script runs on with whatever Work_Zone[0] already holds
                // (research/XiEvents/OpCodes/0x0025.md). A message frame that
                // is still open keeps the ack yield so the host closes it first.
                OP_QUERYWAIT => {
                    if !self.selection_made {
                        if let Some(choice) = self.pending_choice.clone() {
                            return StepResult::AwaitChoice(choice);
                        }
                        if self.message_frame_open() {
                            return StepResult::AwaitMessageAck;
                        }
                        self.exec_pointer += QUERYWAIT_SIZE;
                    } else {
                        self.selection_made = false;
                        self.pending_choice = None;
                        if self.work_zone.lock().unwrap()[0] == CHOICE_CANCELLED {
                            self.finished = true;
                            return StepResult::Cancelled;
                        }
                        self.exec_pointer += QUERYWAIT_SIZE;
                    }
                }
                // XiEvent ReqSet/GetReqStatus family (research/XiEvents/OpCodes/
                // 0x0027.md, 0x0028.md, 0x0029.md, 0x002A.md): actor-choreography
                // sync points. Without a scene there are no actor stacks to push
                // onto or wait on, so they complete instantly; explicit arms
                // because the fallback refuses sets_ret.
                OP_REQSET | OP_REQSET_CHECKED | OP_REQSET_PRIORITY | OP_REQWAIT => {
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                // XiEvent LOADEXTSCHEDULER (research/XiEvents/OpCodes/0x005B.md,
                // 0x0066.md): load an event motion resource into actor1, then
                // SetAction(actor1, key, actor2) unless the key is empty. Retail
                // yields until the resource read completes; that is load latency
                // the host handles asynchronously, so the cue carries the request
                // and execution moves on. 0x66 is the Tpc form: the package number
                // names two containers (A + a CIB-waist-selected B) instead of one
                // banded file id, and an out-of-range package loads nothing.
                // Each load is additionally gated on actor1's entity Type byte
                // (0x5B: {1,2,7,8}, 0x66: {0,1,6}); a refused load arms nothing.
                OP_LOADEXTSCHEDULER | OP_LOADEXTSCHEDULER2 => {
                    let tpc = op == OP_LOADEXTSCHEDULER2;
                    let operand = self.getworkofs(LOADEXTSCHEDULER_FILE_OFS, 0);
                    let motion = if tpc {
                        tpc_motion_packages(operand).map(ExtSchedulerMotion::Tpc)
                    } else {
                        Some(ExtSchedulerMotion::Event(event_motion_dat_id(operand)))
                    };
                    let key = self.fourcc_at(LOADEXTSCHEDULER_KEY_OFS);
                    let actor1 = ActorLookup(self.eventgetcode2(LOADEXTSCHEDULER_ACTOR1_OFS));
                    let accepted = if tpc {
                        matches!(self.actor_type(actor1), 0 | 1 | 6)
                    } else {
                        matches!(self.actor_type(actor1), 1 | 2 | 7 | 8)
                    };
                    if accepted && key != [0; 4] && key != NO_ACTION_KEY {
                        self.pending_action_starts
                            .push((self.resolve_hold_actor(actor1), key));
                        self.cues.push(EventCue::ExtScheduler {
                            motion,
                            actor1,
                            actor2: ActorLookup(self.eventgetcode2(LOADEXTSCHEDULER_ACTOR2_OFS)),
                            key,
                        });
                    }
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                // XiEvent WAITSCHEDULOR (research/XiEvents/OpCodes/0x0053.md):
                // hold while IsMovingAction(key, actor1, actor2) is true on
                // actor1. The host arms that state as a pending hold the
                // renderer's finish report releases; with nothing armed the
                // wait falls through, which is also retail's path when either
                // actor fails to resolve.
                OP_WAITSCHEDULOR => {
                    let actor = ActorLookup(self.eventgetcode2(WAITSCHEDULOR_ACTOR1_OFS));
                    let key = self.fourcc_at(WAITSCHEDULOR_KEY_OFS);
                    if self.action_running(actor, key) {
                        self.parked_on_action_hold = true;
                        return StepResult::Waiting;
                    }
                    self.parked_on_action_hold = false;
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                // XiEvent WAITMAPSCHEDULOR (research/XiEvents/OpCodes/0x0054.md):
                // retail polls the zone object for the 0x2D routine, not an
                // actor, so this parks on the host's hold for the zone sentinel.
                OP_WAITMAPSCHEDULOR => {
                    let key = self.fourcc_at(WAITSCHEDULOR_KEY_OFS);
                    if self.action_running(ActorLookup::ZONE, key) {
                        self.parked_on_action_hold = true;
                        return StepResult::Waiting;
                    }
                    self.parked_on_action_hold = false;
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                // XiEvent WAITLOADSCHEDULER (research/XiEvents/OpCodes/0x0055.md):
                // same hold, keyed on the actor1/key pair a 0x45 or 0x5B started.
                // The six twins run the identical hold on their own scheduler
                // DAT base, which names no file in this model: the loader that
                // armed the hold already chose the DAT and the routine
                // (research/XiEvents/OpCodes/0x00A0.md and kin).
                OP_WAITLOADSCHEDULER
                | OP_WAITLOADSCHED_TWIN_A0
                | OP_WAITLOADSCHED_TWIN_BC
                | OP_WAITLOADSCHED_TWIN_C6
                | OP_WAITLOADSCHED_TWIN_CE
                | OP_WAITLOADSCHED_TWIN_D1
                | OP_WAITLOADSCHED_TWIN_D6 => {
                    let actor = ActorLookup(self.eventgetcode2(WAITLOADSCHEDULER_ACTOR1_OFS));
                    let key = self.fourcc_at(WAITLOADSCHEDULER_KEY_OFS);
                    if self.action_running(actor, key) {
                        self.parked_on_action_hold = true;
                        return StepResult::Waiting;
                    }
                    self.parked_on_action_hold = false;
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                // 0x56 reads an actor and does nothing with it: a deprecated
                // yield (research/XiEvents/OpCodes/0x0056.md). The zero-length
                // wait spends the frame retail's RetFlag spends.
                OP_ACTOR_NOP => {
                    self.eventgetcode2(ACTOR_NOP_ACTOR_OFS);
                    return self.arm_wait(0.0, OPCODE_META[op as usize].size as usize);
                }
                // 0xC1 kills the named entity's last action and returns it to
                // idle, then yields (research/XiEvents/OpCodes/0x00C1.md): the
                // stop-all the ActorStopAction cue already carries, on the
                // resolved actor, no key. An actor retail's GetActorIndex would
                // drop emits nothing.
                OP_KILL_LAST_ACTION => {
                    let actor = ActorLookup(self.eventgetcode2(KILL_LAST_ACTION_ACTOR_OFS));
                    if self.actor_resolved(actor) {
                        self.cues
                            .push(EventCue::ActorStopAction { actor, key: None });
                    }
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                    return StepResult::Waiting;
                }
                // 0x6E EMOT: the work value's low byte is the emote id, its high
                // byte the variant selector (research/XiEvents/OpCodes/0x006E.md).
                // Arms the same-pass start so a 0x99 in this pass sees the
                // animation running; the host's timed hold from the drained cue
                // takes over from the bridge.
                OP_EMOT => {
                    let actor = ActorLookup(self.eventgetcode2(EMOT_ACTOR_OFS));
                    let value = self.getworkofs(EMOT_VALUE_OFS, 0);
                    self.pending_action_starts
                        .push((self.resolve_hold_actor(actor), EMOTE_ANIMATION_KEY));
                    self.cues.push(EventCue::Emote {
                        actor,
                        emote_id: (value & EMOTE_VALUE_BYTE_MASK) as u16,
                        param: ((value >> EMOTE_VALUE_PARAM_SHIFT) & EMOTE_VALUE_BYTE_MASK) as u16,
                    });
                    self.advance(op);
                }
                // 0x63 PLAYANIM: the event entity's emote from the same work slot
                // (research/XiEvents/OpCodes/0x0063.md).
                OP_PLAYANIM => {
                    let value = self.getworkofs(PLAYANIM_VALUE_OFS, 0);
                    self.pending_action_starts.push((
                        self.resolve_hold_actor(ActorLookup::EVENT_ENTITY),
                        EMOTE_ANIMATION_KEY,
                    ));
                    self.cues.push(EventCue::Emote {
                        actor: ActorLookup::EVENT_ENTITY,
                        emote_id: (value & EMOTE_VALUE_BYTE_MASK) as u16,
                        param: ((value >> EMOTE_VALUE_PARAM_SHIFT) & EMOTE_VALUE_BYTE_MASK) as u16,
                    });
                    self.advance(op);
                }
                // 0x99 ANIMWAIT: hold while the named entity's animation is
                // playing (research/XiEvents/OpCodes/0x0099.md). The host arms
                // the timed hold from the emote DAT's authored routine length;
                // with nothing armed the wait falls through, retail's path when
                // the entity is unresolved.
                OP_ANIMWAIT => {
                    let actor = ActorLookup(self.eventgetcode2(ANIMWAIT_ACTOR_OFS));
                    if self.action_running(actor, EMOTE_ANIMATION_KEY) {
                        self.parked_on_action_hold = true;
                        return StepResult::Waiting;
                    }
                    self.parked_on_action_hold = false;
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                // XiEvent MAPSCHEDULOR (research/XiEvents/OpCodes/0x002D.md): start the
                // zone-level routine `key`, waited on by 0x54. Kuluu resolves `key` out
                // of the current zone's own model DAT (ffxi-dat
                // scheduler::zone_scene_file_id, with the entrance/instance partner and
                // non-model carriers as fallbacks); retail runs it out of the same file
                // via the zone object. The host parks that wait on a pending hold the
                // renderer's finish report releases (the routine's authored length in the
                // file is its deadline); the same-batch bridge covers a 0x54 that runs
                // before this cue drains.
                OP_MAPSCHEDULOR => {
                    let key = self.fourcc_at(MAPSCHEDULOR_KEY_OFS);
                    self.pending_action_starts.push((ActorLookup::ZONE, key));
                    self.cues.push(EventCue::ZoneScheduler {
                        key,
                        actor1: ActorLookup(self.eventgetcode2(WAITSCHEDULOR_ACTOR1_OFS)),
                        actor2: ActorLookup(self.eventgetcode2(MAPSCHEDULOR_ACTOR2_OFS)),
                    });
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                OP_SCHEDULOR => {
                    let actor1 = ActorLookup(self.eventgetcode2(SCHEDULOR_ACTOR1_OFS));
                    let key = self.fourcc_at(SCHEDULOR_KEY_OFS);
                    // The same-pass bridge the other loaders use: a 0x53 that
                    // follows the 0x2C in one pass must see the routine as
                    // running, the way retail's IsMovingAction does
                    // (research/XiEvents/OpCodes/0x0053.md). The host's pending hold from
                    // the drained cue takes over from the bridge.
                    self.pending_action_starts
                        .push((self.resolve_hold_actor(actor1), key));
                    self.cues.push(EventCue::ActorMotion {
                        actor1,
                        actor2: ActorLookup(self.eventgetcode2(SCHEDULOR_ACTOR2_OFS)),
                        key,
                    });
                    self.advance(op);
                }
                // 0xDA: a batch motion loader (see the constant). Scan the
                // 28-byte records and emit one ActorMotion cue per record that
                // carries a printable 4cc key; stop at the first record whose
                // key field is not a 4cc, so the advance lands on the next real
                // instruction. Single-site in the corpus, so the record count
                // comes from the scan, not a header count.
                OP_DA => {
                    let mut n = 0;
                    while n < OP_DA_MAX_RECORDS {
                        let rec = OP_DA_HEADER + n * OP_DA_RECORD;
                        let key = self.fourcc_at(rec + OP_DA_KEY);
                        if !key.iter().all(|&b| b != 0 && (0x20..=0x7e).contains(&b)) {
                            break;
                        }
                        let actor1 = ActorLookup(self.eventgetcode2(rec + OP_DA_ACTOR1));
                        let actor2 = ActorLookup(self.eventgetcode2(rec + OP_DA_ACTOR2));
                        self.cues.push(EventCue::ActorMotion {
                            actor1,
                            actor2,
                            key,
                        });
                        n += 1;
                    }
                    self.exec_pointer += OP_DA_HEADER + n * OP_DA_RECORD;
                }
                OP_LOADEVENTSCHEDULER2 => {
                    let file = dat_id_helper(self.getworkofs(LOADEVENTSCHEDULER2_FILE_OFS, 0));
                    let actor1 = ActorLookup(self.eventgetcode2(LOADEVENTSCHEDULER2_ACTOR1_OFS));
                    let tag = self.fourcc_at(LOADEVENTSCHEDULER2_TAG_OFS);
                    self.pending_action_starts
                        .push((self.resolve_hold_actor(actor1), tag));
                    self.cues.push(EventCue::Scheduler {
                        dat_id: SCHEDULER_DAT_ID_BASE.wrapping_add(file as u32),
                        actor1,
                        actor2: ActorLookup(self.eventgetcode2(LOADEVENTSCHEDULER2_ACTOR2_OFS)),
                        tag,
                        duration: self.getworkofs(LOADEVENTSCHEDULER2_DURATION_OFS, 0) as u16,
                    });
                    self.advance(op);
                }
                // The 0x45 twins: the same helper and layout, but each on its
                // own DAT base and with the work value used raw — the
                // `dat_id_helper` remap is the 0x45-only branch of the helper
                // (research/XiEvents/OpCodes/0x0045.md, 0x0062.md and kin).
                OP_SCHED_TWIN_62 | OP_SCHED_TWIN_9F | OP_SCHED_TWIN_BB | OP_SCHED_TWIN_C5
                | OP_SCHED_TWIN_CD | OP_SCHED_TWIN_D0 | OP_SCHED_TWIN_D5 => {
                    let base = scheduler_twin_base(op).expect("matched a 0x45 twin");
                    let file = self.getworkofs(LOADEVENTSCHEDULER2_FILE_OFS, 0) as u32;
                    let actor1 = ActorLookup(self.eventgetcode2(LOADEVENTSCHEDULER2_ACTOR1_OFS));
                    let tag = self.fourcc_at(LOADEVENTSCHEDULER2_TAG_OFS);
                    self.pending_action_starts
                        .push((self.resolve_hold_actor(actor1), tag));
                    self.cues.push(EventCue::Scheduler {
                        dat_id: base + file,
                        actor1,
                        actor2: ActorLookup(self.eventgetcode2(LOADEVENTSCHEDULER2_ACTOR2_OFS)),
                        tag,
                        duration: self.getworkofs(LOADEVENTSCHEDULER2_DURATION_OFS, 0) as u16,
                    });
                    self.advance(op);
                }
                // The end-scheduler family: kill the tag-named action on
                // actor1 (research/XiEvents/OpCodes/0x0050.md, 0x0051.md,
                // 0x0052.md). Retail skips the kill when either actor does not
                // resolve; this VM resolves reserved lookups to the event
                // entity, so the cue goes out unconditionally and the consumer
                // drops an unknown id, the way the other actor cues do.
                OP_ENDSCHEDULOR | OP_ENDMAPSCHEDULOR => {
                    let actor = ActorLookup(self.eventgetcode2(ENDSCHEDULOR_ACTOR1_OFS));
                    self.cues.push(EventCue::ActorStopAction {
                        actor,
                        key: stop_action_key(self.fourcc_at(ENDSCHEDULOR_TAG_OFS)),
                    });
                    self.advance(op);
                }
                OP_ENDLOADSCHEDULER_MAIN
                | OP_ENDLOADSCHED_TWIN_A1
                | OP_ENDLOADSCHED_TWIN_A3
                | OP_ENDLOADSCHED_TWIN_BD
                | OP_ENDLOADSCHED_TWIN_C7
                | OP_ENDLOADSCHED_TWIN_CF
                | OP_ENDLOADSCHED_TWIN_D2
                | OP_ENDLOADSCHED_TWIN_D7 => {
                    let actor = ActorLookup(self.eventgetcode2(ENDLOADSCHED_ACTOR1_OFS));
                    self.cues.push(EventCue::ActorStopAction {
                        actor,
                        key: stop_action_key(self.fourcc_at(ENDLOADSCHED_TAG_OFS)),
                    });
                    self.advance(op);
                }
                // The cast is the spell effect DAT for the work operand's
                // animation index, its `main` routine on actor1 with actor2 as
                // the target: the same file and routine a 0x028 magic finish
                // plays, so the 0x45 cue carries it unchanged. No hold entry:
                // retail's 0x73 arms no wait of its own and the scripts that
                // author it cover the cast with a 0x1C WAIT
                // (research/XiEvents/OpCodes/0x0073.md). A work operand outside
                // u16 names no file and plays nothing, like retail's
                // unresolved-actor early return.
                OP_MAGICSCHEDULOR => {
                    let actor1 = ActorLookup(self.eventgetcode2(MAGICSCHEDULOR_ACTOR1_OFS));
                    let actor2 = ActorLookup(self.eventgetcode2(MAGICSCHEDULOR_ACTOR2_OFS));
                    if let Ok(animation) = u16::try_from(self.getworkofs(MAGICSCHEDULOR_KEY_OFS, 0))
                    {
                        self.cues.push(EventCue::Scheduler {
                            dat_id: MAGIC_DAT_ID_BASE + u32::from(animation),
                            actor1,
                            actor2,
                            tag: MAGIC_ROUTINE_TAG,
                            duration: SCHEDULER_DURATION_FROM_DAT,
                        });
                    }
                    self.advance(op);
                }
                // The 0x73 twin: the same spell cast, but the case byte at +1
                // shifts the key to +2 and actor2 to +8 and widens the advance
                // to 12. Cases 0/1/2 all run the `main` routine; a case past 2
                // arms no cast (retail's empty switch fall-through) yet still
                // advances the full width (research/XiEvents/OpCodes/0x00C4.md).
                OP_MAGIC_TWIN => {
                    let case = self.byte_at(MAGIC_TWIN_CASE_OFS);
                    let actor1 = ActorLookup(self.eventgetcode2(MAGIC_TWIN_ACTOR1_OFS));
                    let actor2 = ActorLookup(self.eventgetcode2(MAGIC_TWIN_ACTOR2_OFS));
                    if case <= 2 {
                        if let Ok(animation) = u16::try_from(self.getworkofs(MAGIC_TWIN_KEY_OFS, 0))
                        {
                            self.cues.push(EventCue::Scheduler {
                                dat_id: MAGIC_DAT_ID_BASE + u32::from(animation),
                                actor1,
                                actor2,
                                tag: MAGIC_ROUTINE_TAG,
                                duration: SCHEDULER_DURATION_FROM_DAT,
                            });
                        }
                    }
                    self.exec_pointer += MAGIC_TWIN_SIZE;
                }
                // 0x7D runs the work-operand scheduler on the local player with
                // the player as its own target — the rank-up animations
                // (research/XiEvents/OpCodes/0x007D.md).
                OP_LOCAL_PLAYER_SCHEDULER => {
                    let file = self.getworkofs(LOCAL_PLAYER_SCHEDULER_FILE_OFS, 0) as u32;
                    self.cues.push(EventCue::Scheduler {
                        dat_id: LOCAL_PLAYER_SCHEDULER_DAT_ID_BASE + file,
                        actor1: ActorLookup::LOCAL_PLAYER,
                        actor2: ActorLookup::LOCAL_PLAYER,
                        tag: MAGIC_ROUTINE_TAG,
                        duration: SCHEDULER_DURATION_FROM_DAT,
                    });
                    self.advance(op);
                }
                // 0x32 MainSpeed: arm the speed the non-scene `OP_MOVE` case 0
                // carries; the scene path keeps the same value on its `Scene`
                // (research/XiEvents/OpCodes/0x0032.md).
                OP_MAIN_SPEED => {
                    self.move_speed = self.getworkofs(MAIN_SPEED_OFS, 0);
                    self.advance(op);
                }
                // 0x31 SMOVE: mode 0 arms the goal (work slots 2/4/6, event
                // units) and the MoveTime budget (slot 8, seconds); mode 1
                // walks the event entity to that goal at the 0x32 speed and
                // parks until the move hold releases it. The cue carries the
                // budget so the host caps the distance-derived hold to it
                // (research/XiEvents/OpCodes/0x0031.md).
                OP_SMOVE => match self.byte_at(1) {
                    0x00 => {
                        self.smove_goal = Some(self.position_operands(2, false));
                        self.smove_time = self.getworkofs(8, 0) as f32 * 0.001;
                        self.smove_started = false;
                        self.exec_pointer += 10;
                    }
                    0x01 => {
                        if !self.smove_started {
                            if let Some(goal) = self.smove_goal {
                                let actor = ActorLookup::EVENT_ENTITY;
                                self.pending_move_starts
                                    .push(self.resolve_hold_actor(actor));
                                self.cues.push(EventCue::ActorMove {
                                    actor,
                                    goal,
                                    speed: self.move_speed,
                                    max_time: (self.smove_time > 0.0).then_some(self.smove_time),
                                });
                                self.smove_started = true;
                            }
                        }
                        if self.move_running(ActorLookup::EVENT_ENTITY) {
                            self.parked_on_move_hold = true;
                            return StepResult::Waiting;
                        }
                        self.parked_on_move_hold = false;
                        self.exec_pointer += 2;
                    }
                    _ => return StepResult::Unimplemented(op),
                },
                // 0x72 GETWEATHER: mode 0 kicks off the forecast read and mode 1
                // copies three values into Work_Zone[2..5)
                // (research/XiEvents/OpCodes/0x0072.md). The table is resident
                // in kuluu (the host loads it once), so the read is instant:
                // mode 0 advances past itself and yields one frame the way
                // retail's async read does, and mode 1 reads the values and
                // advances. A region or index the shipped table does not cover
                // writes nothing, like retail's unresolved early return.
                OP_GETWEATHER => match self.byte_at(1) {
                    0x00 => {
                        self.exec_pointer += 4;
                        return self.arm_wait(0.0, 0);
                    }
                    0x01 => {
                        if let Some(forecast) = &self.weather_forecast {
                            let region = self.getworkofs(2, 0) as u32;
                            let day = self.getworkofs(4, 0) as u32;
                            if let Some([v0, v1, v2]) = forecast.values(region, day) {
                                let mut zone = self.work_zone.lock().unwrap();
                                zone[2] = v0;
                                zone[3] = v1;
                                zone[4] = v2;
                            }
                        }
                        self.exec_pointer += 6;
                    }
                    _ => return StepResult::Unimplemented(op),
                },
                // 0x82 RANGE_RECT: find the zone range rect named by the u32 at
                // +1 and hit-test the event entity's tracked position against it;
                // a hit advances 7, a miss (no rect, no position, or outside) jumps
                // to the u16 at +5 (research/XiEvents/OpCodes/0x0082.md).
                OP_RANGE_RECT => {
                    let rect_id = self.eventgetcode2(1);
                    if self.range_rect_hit(rect_id) {
                        self.exec_pointer += 7;
                    } else {
                        self.exec_pointer = self.eventgetcode(5) as usize;
                    }
                }
                // 0x39 SetFacing: set the event entity's facing, the raw work
                // value on the 0..4095 heading scale (research/XiEvents/OpCodes/
                // 0x0039.md).
                OP_SET_FACING => {
                    let heading = self.getworkofs(SET_FACING_OFS, 0);
                    self.cues.push(EventCue::ActorFace {
                        actor: ActorLookup::EVENT_ENTITY,
                        heading,
                    });
                    self.advance(op);
                }
                // 0x4B: turn the named actor to the work-operand yaw
                // (research/XiEvents/OpCodes/0x004B.md).
                OP_YAW => {
                    let actor = ActorLookup(self.eventgetcode2(YAW_ACTOR_OFS));
                    let heading = self.getworkofs(YAW_HEADING_OFS, 0);
                    self.cues.push(EventCue::ActorFace { actor, heading });
                    self.advance(op);
                }
                // 0x36: place the event entity at the work-operand position, no
                // heading (research/XiEvents/OpCodes/0x0036.md).
                OP_SET_EVENT_POS => {
                    let position = self.position_operands(SET_EVENT_POS_X_OFS, false);
                    self.cues.push(EventCue::ActorPlace {
                        actor: ActorLookup::EVENT_ENTITY,
                        position,
                    });
                    self.advance(op);
                }
                // 0xBA: place the named actor at the work-operand position and
                // heading (research/XiEvents/OpCodes/0x00BA.md).
                OP_SET_ACTOR_POS => {
                    let actor = ActorLookup(self.eventgetcode2(SET_ACTOR_POS_ACTOR_OFS));
                    let position = self.position_operands(SET_ACTOR_POS_X_OFS, true);
                    self.cues.push(EventCue::ActorPlace { actor, position });
                    self.advance(op);
                }
                // Case 2 queries the camera state into a work slot rather than
                // changing it, and every other case is retail's no-op
                // fall-through (research/XiEvents/OpCodes/0x0046.md).
                OP_DEFCAMERA => {
                    match self.byte_at(DEFCAMERA_CASE_OFS) {
                        DEFCAMERA_CASE_LOCK => self.cues.push(EventCue::CameraLock { lock: true }),
                        DEFCAMERA_CASE_UNLOCK => {
                            self.cues.push(EventCue::CameraLock { lock: false })
                        }
                        _ => {}
                    }
                    self.advance(op);
                }
                // 0x20 writes retail's CliEventUcFlag; while it holds, the
                // player's CanIMove is false (research/XiEvents/OpCodes/0x0020.md).
                OP_PLAYER_CONTROL => {
                    self.cues.push(EventCue::PlayerControl {
                        locked: self.byte_at(PLAYER_CONTROL_FLAG_OFS) != 0,
                    });
                    self.advance(op);
                }
                OP_EVENTHIDE => {
                    self.cues.push(EventCue::ActorHide {
                        target: ActorLookup(self.eventgetcode2(EVENTHIDE_TARGET_OFS)),
                        hide: self.byte_at(EVENTHIDE_FLAG_OFS) & EVENTHIDE_FLAG_MASK != 0,
                    });
                    self.advance(op);
                }
                // 0x6C fades the target's alpha to the work(5) byte over the
                // work(7) frames and parks the script for that fade
                // (research/XiEvents/OpCodes/0x006C.md).
                OP_TRANSPAR => {
                    let actor = ActorLookup(self.eventgetcode2(TRANSPAR_ACTOR_OFS));
                    let end_alpha = self.getworkofs(TRANSPAR_ALPHA_OFS, 0);
                    let frames = self.getworkofs(TRANSPAR_TIME_OFS, 0).max(1);
                    self.cues.push(EventCue::Transpar {
                        actor,
                        end_alpha,
                        duration_frames: frames,
                    });
                    return self.arm_wait(frames as f32, TRANSPAR_SIZE);
                }
                // 0xC8 opens the map window on the work-slot zone id —
                // research/XiEvents/OpCodes/0x00C8.md.
                OP_MAP_TUTORIAL => {
                    self.cues.push(EventCue::MapOpen {
                        map_id: self.getworkofs(MAP_OPEN_ID_OFS, 0),
                        tutorial: self.getworkofs(MAP_OPEN_TUTORIAL_OFS, 0) & 1 != 0,
                    });
                    self.advance(op);
                }
                // 0x8B places a named marker on the player's map —
                // research/XiEvents/OpCodes/0x008B.md.
                OP_MAP_MARKER => {
                    let mut name = [0u8; 16];
                    for (slot, byte) in name.iter_mut().enumerate() {
                        *byte = self.byte_at(MAP_MARKER_NAME_OFS + slot);
                    }
                    // Retail rewrites underscores to spaces before the rename
                    // (research/XiEvents/OpCodes/0x008B.md).
                    for b in &mut name {
                        if *b == b'_' {
                            *b = b' ';
                        }
                    }
                    self.cues.push(EventCue::MapMarker {
                        map_id: self.getworkofs(MAP_MARKER_ID_OFS, 0),
                        x_milli: self.getworkofs(MAP_MARKER_X_OFS, 0),
                        y_milli: self.getworkofs(MAP_MARKER_Y_OFS, 0),
                        name,
                    });
                    self.advance(op);
                }
                // 0x89 opens the map on the work-slot zone id, sub-menus
                // hidden (research/XiEvents/OpCodes/0x0089.md).
                OP_OPEN_MAP => {
                    self.cues.push(EventCue::MapOpen {
                        map_id: self.getworkofs(OPEN_MAP_ID_OFS, 0),
                        tutorial: false,
                    });
                    self.advance(op);
                }
                // 0x8D opens the map with its authored sub-menu property, which
                // the MapOpen carrier does not carry
                // (research/XiEvents/OpCodes/0x008D.md).
                OP_OPEN_MAP_PROPS => {
                    self.cues.push(EventCue::MapOpen {
                        map_id: self.getworkofs(OPEN_MAP_ID_OFS, 0),
                        tutorial: false,
                    });
                    self.advance(op);
                }
                // 0xB8 adds a named marker to the map; with the map closed
                // retail opens it data-level and closes it again, so the
                // visible result is the marker itself
                // (research/XiEvents/OpCodes/0x00B8.md).
                OP_MAP_ADD_MARK => {
                    let mut name = [0u8; 16];
                    for (slot, byte) in name.iter_mut().enumerate() {
                        *byte = self.byte_at(MAP_ADD_MARK_NAME_OFS + slot);
                    }
                    // Retail rewrites underscores to spaces before the rename
                    // (research/XiEvents/OpCodes/0x00B8.md).
                    for b in &mut name {
                        if *b == b'_' {
                            *b = b' ';
                        }
                    }
                    self.cues.push(EventCue::MapMarker {
                        map_id: self.getworkofs(MAP_ADD_MARK_ID_OFS, 0),
                        x_milli: self.getworkofs(MAP_ADD_MARK_X_OFS, 0),
                        y_milli: self.getworkofs(MAP_ADD_MARK_Y_OFS, 0),
                        name,
                    });
                    self.advance(op);
                }
                OP_CLOSE_MAP => {
                    self.cues.push(EventCue::MapClose);
                    self.advance(op);
                }
                // 0xD4 MAP_QUERY: cases 0 and 2 run the 0x24 query helper —
                // case 0 also opens the current zone's map (type 6) — and park
                // on the answer; cases 1/3/4/5 copy into the client's query
                // window, which has no kuluu counterpart, so they advance by
                // their width; an unknown case parks
                // (research/XiEvents/OpCodes/0x00D4.md).
                OP_MAP_QUERY => {
                    let sub = self.byte_at(1);
                    match sub {
                        0 | 2 => {
                            if self.selection_made {
                                self.selection_made = false;
                                self.pending_choice = None;
                                if self.work_zone.lock().unwrap()[0] == CHOICE_CANCELLED {
                                    self.finished = true;
                                    return StepResult::Cancelled;
                                }
                                self.exec_pointer += 8;
                            } else {
                                self.arm_map_query_choice();
                                if sub == 0 {
                                    self.cues.push(EventCue::MapOpen {
                                        map_id: self.current_zone,
                                        tutorial: false,
                                    });
                                }
                                return match self.pending_choice.clone() {
                                    Some(choice) => StepResult::AwaitChoice(choice),
                                    None => StepResult::AwaitMessageAck,
                                };
                            }
                        }
                        1 => {
                            self.exec_pointer += 8;
                        }
                        3 => {
                            self.exec_pointer += 6;
                        }
                        4 | 5 => {
                            self.exec_pointer += 12;
                        }
                        _ => return StepResult::Unimplemented(op),
                    }
                }
                // 0xB3 RANKING: the ranking-board cases. LSB has no ranking
                // handler, so the read cases write zeros into the board's work
                // slots (the board draws empty) and every case advances by its
                // width; no packet is sent (research/XiEvents/OpCodes/0x00B3.md).
                OP_RANKING => {
                    let sub = self.byte_at(1);
                    match sub {
                        1 => {
                            for ofs in [2usize, 4, 6, 8, 10, 12] {
                                self.setworkofs(ofs, 0, 0);
                            }
                            self.exec_pointer += 14;
                        }
                        5 => {
                            for ofs in [2usize, 4, 6, 8, 10, 12, 14, 16] {
                                self.setworkofs(ofs, 0, 0);
                            }
                            self.exec_pointer += 18;
                        }
                        9 => {
                            self.setworkofs(2, 0, 0);
                            self.exec_pointer += 4;
                        }
                        0 | 3 | 4 | 6 | 7 => {
                            self.exec_pointer += 4;
                        }
                        _ => {
                            self.exec_pointer += 2;
                        }
                    }
                }
                // 0x67's operands are retail's event-message presets, which carry no cue
                // payload (research/XiEvents/OpCodes/0x0067.md).
                OP_HIDE_HUD => {
                    self.cues.push(EventCue::HudHide { hide: true });
                    self.advance(op);
                }
                OP_SHOW_HUD => {
                    self.cues.push(EventCue::HudHide { hide: false });
                    self.advance(op);
                }
                // 0x77 holds the game clock at its authored hour; a sentinel operand means no
                // time change, and the weather half is server-driven here
                // (research/XiEvents/OpCodes/0x0077.md).
                OP_STOP_CLOCK => {
                    let hour = self.getworkofs(STOP_CLOCK_HOUR_OFS, 0);
                    if hour != STOP_CLOCK_NO_HOUR {
                        self.cues.push(EventCue::ClockHold {
                            stop: true,
                            hour: Some(hour.rem_euclid(24) as u32),
                            minute: 0,
                            day_from_epoch: None,
                        });
                    }
                    self.advance(op);
                }
                OP_RESTORE_CLOCK => {
                    self.cues.push(EventCue::ClockHold {
                        stop: false,
                        hour: None,
                        minute: 0,
                        day_from_epoch: None,
                    });
                    self.advance(op);
                }
                // 0xA9 zeros the local time, then jumps the date to Vana day
                // 7 * work[1] at 00:30 (research/XiEvents/OpCodes/0x00A9.md
                // Helper2, SetMinute).
                OP_SET_CLOCK_DATE => {
                    let day = self.getworkofs(SET_CLOCK_DATE_DAY_OFS, 0);
                    self.cues.push(EventCue::ClockHold {
                        stop: true,
                        hour: Some(SET_CLOCK_DATE_HOUR),
                        minute: SET_CLOCK_DATE_MINUTE,
                        day_from_epoch: Some(day.saturating_mul(7).max(0) as u32),
                    });
                    self.advance(op);
                }
                // 0xC9 releases the hold 0x77/0xA9 set
                // (research/XiEvents/OpCodes/0x00C9.md).
                OP_ENABLE_TIMER => {
                    self.cues.push(EventCue::ClockHold {
                        stop: false,
                        hour: None,
                        minute: 0,
                        day_from_epoch: None,
                    });
                    self.advance(op);
                }
                // 0x5C MUSIC: the low band (0x00-0x07) sets BGM slot `sub`'s
                // song to the +2 work selector and starts it at full volume;
                // the 0x80-0x87 band does the same for slot `sub & 7` at the
                // +4 start volume; 0xA0/0xA1 ease the playing track to the +2
                // volume over the +4 frames, the 0x5D shape
                // (research/XiEvents/OpCodes/0x005C.md).
                OP_MUSIC => {
                    let sub = self.byte_at(1);
                    match sub {
                        0x00..=0x07 => {
                            self.cues.push(EventCue::MusicSong {
                                slot: sub,
                                track: self.getworkofs(MUSIC_SONG_TRACK_OFS, 0) as u16,
                                volume: MUSIC_VOLUME_MAX,
                            });
                            self.exec_pointer += 4;
                        }
                        0x80..=0x87 => {
                            self.cues.push(EventCue::MusicSong {
                                slot: sub & MUSIC_SONG_SLOT_MASK,
                                track: self.getworkofs(MUSIC_SONG_TRACK_OFS, 0) as u16,
                                volume: self.getworkofs(MUSIC_SONG_VOLUME_OFS, 0).clamp(0, 255)
                                    as u8,
                            });
                            self.exec_pointer += 6;
                        }
                        0xA0 | 0xA1 => {
                            self.cues.push(EventCue::MusicVolume {
                                volume: self
                                    .getworkofs(MUSIC_SONG_TRACK_OFS, 0)
                                    .clamp(0, MUSIC_VOLUME_MAX as i32)
                                    as u8,
                                fade_frames: self.getworkofs(MUSIC_SONG_VOLUME_OFS, 0) as u16,
                            });
                            self.exec_pointer += 6;
                        }
                        _ => return StepResult::Unimplemented(op),
                    }
                }
                OP_MUSICVOLUME => {
                    self.cues.push(EventCue::MusicVolume {
                        volume: self
                            .getworkofs(MUSICVOLUME_LEVEL_OFS, 0)
                            .clamp(0, MUSIC_VOLUME_MAX as i32)
                            as u8,
                        fade_frames: self.getworkofs(MUSICVOLUME_FADE_OFS, 0) as u16,
                    });
                    self.advance(op);
                }
                // 0x69 sets the named sound types to full volume or mutes them
                // (the flag byte); 0x6A eases them to work[1] * 0.001 over
                // work[3] frames (research/XiEvents/OpCodes/0x0069.md, 0x006A.md).
                OP_SET_SOUND_VOLUME => {
                    let mask = self.getworkofs(SET_SOUND_MASK_OFS, 0) as u8;
                    let volume = if self.byte_at(SET_SOUND_FLAG_OFS) == 0 {
                        MUSIC_VOLUME_MAX
                    } else {
                        0
                    };
                    self.cues.push(EventCue::SoundVolume {
                        mask,
                        volume,
                        fade_frames: 0,
                    });
                    self.advance(op);
                }
                OP_CHANGE_SOUND_VOLUME => {
                    let mask = self.getworkofs(CHANGE_SOUND_MASK_OFS, 0) as u8;
                    let volume = (self.getworkofs(CHANGE_SOUND_LEVEL_OFS, 0).clamp(0, 1000)
                        * MUSIC_VOLUME_MAX as i32
                        / 1000) as u8;
                    self.cues.push(EventCue::SoundVolume {
                        mask,
                        volume,
                        fade_frames: self.getworkofs(CHANGE_SOUND_FADE_OFS, 0) as u16,
                    });
                    self.advance(op);
                }
                // 0x4C/0x4D/0x4F write the event entity's StatusEvent: the door's
                // open/close byte and the M1..M8 event-motion range
                // (research/XiEvents/OpCodes/0x004C.md, 0x004D.md, 0x004F.md;
                // research/XIClient/src/XIClient/include/World/Actor/GameStatus.h).
                // Each is gated on a Render.Flags0 bit no tier names; the door
                // consumer's change-dedup is the modelled equivalent, and the
                // cue rides 0x7E's Mount shape — the same field, so the whole
                // path is already there.
                OP_DOOR_OPEN => {
                    self.emit_status_event_cue(STATUS_EVENT_DOOR_OPEN);
                    self.advance(op);
                }
                OP_DOOR_CLOSE => {
                    self.emit_status_event_cue(STATUS_EVENT_DOOR_CLOSE);
                    self.advance(op);
                }
                OP_STATUS_EVENT => {
                    let status = self
                        .getworkofs(STATUS_EVENT_VALUE_OFS, 0)
                        .wrapping_add(STATUS_EVENT_MOTION_BASE as i32)
                        as u8;
                    self.emit_status_event_cue(status);
                    self.advance(op);
                }
                OP_DOOR_OPEN2 => {
                    self.emit_status_event_cue(STATUS_EVENT_DOOR_OPEN2);
                    self.advance(op);
                }
                OP_DOOR_CLOSE2 => {
                    self.emit_status_event_cue(STATUS_EVENT_DOOR_CLOSE2);
                    self.advance(op);
                }
                // The Flags1 half of 0x90 has no tier-named meaning, so the cue
                // carries only the hide write (research/XiEvents/OpCodes/0x0090.md).
                OP_EVENT_HIDE_ALWAYS => {
                    self.cues.push(EventCue::ActorHide {
                        target: ActorLookup::EVENT_ENTITY,
                        hide: true,
                    });
                    self.advance(op);
                }
                // XiEvent CHOCOBO (research/XiEvents/OpCodes/0x007E.md): puts an
                // actor on or off a mount mid-cutscene. Its width is its case
                // byte's; refusing it auto-released the rental cutscene one
                // opcode past 0x53.
                OP_CHOCOBO => {
                    let Some(width) = self
                        .event_data
                        .get(self.exec_pointer + 1)
                        .and_then(|&sub| crate::opcode_meta::sub_size(op, sub))
                    else {
                        return StepResult::Unimplemented(op);
                    };
                    self.emit_mount_cue();
                    self.exec_pointer += width as usize;
                }
                // The load/turn waits: retail yields only once the named entity
                // resolves and reports mid-load/mid-turn, and this VM resolves no
                // actors, so each takes its own "no such entity" advance
                // (research/XiEvents/OpCodes/0x0080.md, 0x0076.md).
                OP_LOADWAIT | OP_TURNCHECK => self.exec_pointer += LOADWAIT_SIZE,
                OP_MAPLOAD | OP_MAPLOAD_KEEP => self.exec_pointer += MAPLOAD_SIZE,
                OP_MUSICREADWAIT | OP_YIELD => self.exec_pointer += YIELD_SIZE,
                // 0x98 yields while the zone is reading ext data; an event
                // never starts before the zone is resident, so only the
                // advance path is reachable (research/XiEvents/OpCodes/0x0098.md).
                OP_ZONE_READ_YIELD => self.exec_pointer += OPCODE_META[op as usize].size as usize,
                // 0x9B yields while the event entity is playing any animation
                // (research/XiEvents/OpCodes/0x009B.md): the same hold the
                // loader waits arm, read for the event entity against every
                // key, retail's AnimationPlay being a per-entity flag, not a
                // routine slot.
                OP_ANIM_YIELD => {
                    if self.entity_animation_running() {
                        self.parked_on_action_hold = true;
                        return StepResult::Waiting;
                    }
                    self.parked_on_action_hold = false;
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                // 0x26 sets RetFlag and never advances: a deprecated yield that
                // spins in place until the event ends by another route
                // (research/XiEvents/OpCodes/0x0026.md).
                OP_YIELD_FOREVER => return StepResult::Waiting,
                OP_BITTEST => self.op_bit_test(op),
                // 0x44 tests whether the entity the work operand names is in
                // the pool and branches on it: an if without an else body, the
                // else-target skipping the true side
                // (research/XiEvents/OpCodes/0x0044.md).
                OP_ENTITY_VALID => {
                    let id = self.getworkofs(ENTITY_VALID_ID_OFS, 0) as u32;
                    if self.entity_in_pool(id) {
                        self.exec_pointer += OPCODE_META[op as usize].size as usize;
                    } else {
                        self.exec_pointer = self.eventgetcode(ENTITY_VALID_TARGET_OFS) as usize;
                    }
                }
                // 0x007F is 0x25 QUERYWAIT with one difference: a cancelled
                // menu stores 255 and runs on rather than ending the event
                // (research/XiEvents/OpCodes/0x007F.md).
                OP_QUERYWAIT2 => {
                    if !self.selection_made {
                        if let Some(choice) = self.pending_choice.clone() {
                            return StepResult::AwaitChoice(choice);
                        }
                        if self.message_frame_open() {
                            return StepResult::AwaitMessageAck;
                        }
                        self.exec_pointer += QUERYWAIT_SIZE;
                    } else {
                        self.selection_made = false;
                        self.pending_choice = None;
                        if self.work_zone.lock().unwrap()[0] == CHOICE_CANCELLED {
                            self.work_zone.lock().unwrap()[0] = CHOICE_CANCELLED_QUERYWAIT2;
                        }
                        self.exec_pointer += QUERYWAIT_SIZE;
                    }
                }
                // The sub-byte-dispatched families. Their width is the case's,
                // not the table's widest, so an undocumented sub stops the VM
                // rather than falling back and landing mid-instruction.
                OP_LOOKSET | OP_LOADROOM | OP_ITEMINFO | OP_ENTITYSPEED | OP_MOVE | OP_WINDOW
                | OP_MENU | OP_RENDERFLAG | OP_REQRESET | OP_STRINGOPS | OP_NAMESET
                | OP_SUBSCHED | OP_STATUSSET => {
                    let Some(&sub) = self.event_data.get(self.exec_pointer + 1) else {
                        return StepResult::Unimplemented(op);
                    };
                    // The player-input polls have no width this VM may take —
                    // see [`crate::opcode_meta::is_input_wait`].
                    if crate::opcode_meta::is_input_wait(op, sub) {
                        return StepResult::Unimplemented(op);
                    }
                    let Some(width) = crate::opcode_meta::sub_size(op, sub) else {
                        return StepResult::Unimplemented(op);
                    };
                    match (op, sub) {
                        // 0xB4 case 0: the inline 16-byte string at +4 becomes
                        // the work string the +2 operand selects
                        // (research/XiEvents/OpCodes/0x00B4.md).
                        (OP_WINDOW, 0x00) => {
                            let mut name = [0u8; 16];
                            let start = self.exec_pointer + 4;
                            let end = (start + 16).min(self.event_data.len());
                            name[..end - start].copy_from_slice(&self.event_data[start..end]);
                            self.setworkstr(2, name);
                        }
                        // [`OP_WINDOW`] case 1: the PENDINGSTR table entry the +4
                        // work operand selects becomes that work string; an
                        // out-of-range index reads slot 0, retail's clamp
                        // (research/XiEvents/OpCodes/0x00B4.md).
                        (OP_WINDOW, 0x01) => {
                            let idx = self.getworkofs(4, 0);
                            let idx = if (0..4).contains(&idx) {
                                idx as usize
                            } else {
                                0
                            };
                            self.setworkstr(2, self.pending_strings[idx]);
                        }
                        // 0xB5 case 0: the event entity's display name becomes
                        // the work string the +2 operand selects
                        // (research/XiEvents/OpCodes/0x00B5.md).
                        (OP_NAMESET, 0x00) => {
                            self.cues.push(EventCue::EntityName {
                                actor: ActorLookup::EVENT_ENTITY,
                                name: self.getworkstr(2),
                            });
                        }
                        // 0x1F case 0: walk the event entity to its goal at the
                        // 0x32 speed; case 1 holds while that move still has
                        // frames left (research/XiEvents/OpCodes/0x001F.md).
                        (OP_MOVE, 0x00) => {
                            let goal = self.position_operands(MOVE_GOAL_X_OFS, false);
                            self.cues.push(EventCue::ActorMove {
                                actor: ActorLookup::EVENT_ENTITY,
                                goal,
                                speed: self.move_speed,
                                max_time: None,
                            });
                        }
                        (OP_MOVE, 0x01) => {
                            if self.move_running(ActorLookup::EVENT_ENTITY) {
                                self.parked_on_move_hold = true;
                                return StepResult::Waiting;
                            }
                            self.parked_on_move_hold = false;
                        }
                        _ => {}
                    }
                    self.exec_pointer += width as usize;
                }
                // The retail two-flag gate (CliEventCancelSetFlag) is empirically
                // open on every event that uses these opcodes — the locked-in
                // cutscenes disarm and stay disarmed, plain conversations do not
                // touch it — so model the flag directly
                // (research/XiEvents/OpCodes/0x0042.md, 0x002E.md).
                OP_CANCEL_DISARM => {
                    self.cancel_armed = false;
                    self.advance(op);
                }
                OP_CANCEL_ARM => {
                    self.cancel_armed = true;
                    self.advance(op);
                }
                _ => {
                    let meta = OPCODE_META.get(op as usize).copied();
                    match meta {
                        Some(m) if m.valid && !m.jumps && !m.sets_ret && m.size > 0 => {
                            self.exec_pointer += self.op_width(op, m.size) as usize;
                        }
                        _ => return StepResult::Unimplemented(op),
                    }
                }
            }
        }
    }

    /// Bytes to step past `op`, honouring the sub-selector for the opcodes whose
    /// width it decides (see [`crate::opcode_meta::sub_size`]).
    fn op_width(&self, op: u8, fixed: u8) -> u8 {
        self.event_data
            .get(self.exec_pointer + 1)
            .and_then(|&sub| crate::opcode_meta::sub_size(op, sub))
            .unwrap_or(fixed)
    }

    fn advance(&mut self, op: u8) {
        let fixed = OPCODE_META[op as usize].size;
        self.exec_pointer += self.op_width(op, fixed) as usize;
    }

    /// Yield on the frame [`Self::open_message`] just opened. Retail prints the
    /// line at the EventMess opcode (CliEventMessOpenFlag goes up there) and only
    /// parks at MESWAIT, so a yield between the two must not hold the frame back
    /// (research/XiEvents/OpCodes/0x001D.md, 0x0023.md).
    fn emit_open_message(&self) -> StepResult {
        match self.pending_message.clone() {
            Some(msg) => StepResult::AwaitMessage(msg),
            None => StepResult::AwaitMessageAck,
        }
    }

    /// Set `CliEventMessOpenFlag` and hold the message for the MESWAIT (0x23)
    /// that yields it — the shape every message opcode shares.
    fn open_message(&mut self, message_id: u32, speaker_index: Option<u16>) {
        self.message_open = MESSAGE_OPEN_AWAITING;
        self.pending_message = Some(EventMessage {
            message_id,
            speaker_index,
            params: self.params(),
        });
    }

    /// `XiEvent::GetActorIndex` (research/XiEvents/Event VM Functions.md): the
    /// target index a baked entity lookup selects. The reserved lookups index a
    /// host entity table (local player, party slots) this dialog-only VM does
    /// not model, so they resolve to the event entity; retail instead drops the
    /// whole line when a lookup fails, which is the failure mode this VM exists
    /// to avoid.
    fn actor_index(&self, lookup: u32) -> u16 {
        ActorLookup(lookup)
            .target_index()
            .unwrap_or(self.speaker_index)
    }

    /// The four ASCII operand bytes at `ExecPointer + index`, in file order —
    /// the scheduler/action keys are tags, not numbers.
    fn fourcc_at(&self, index: usize) -> FourCc {
        self.eventgetcode2(index).to_le_bytes()
    }

    /// The door opcodes' `StatusEvent` write, on 0x7E's Mount cue: same field,
    /// so the session and wire paths 0x7E already owns carry it
    /// (research/XiEvents/OpCodes/0x004C.md).
    fn emit_status_event_cue(&mut self, status_event: u8) {
        self.cues.push(EventCue::Mount {
            target: ActorLookup::EVENT_ENTITY,
            status_event,
            mount_id: None,
        });
    }

    /// The `StatusEvent` write 0x7E's case performs, as a cue
    /// (research/XiEvents/OpCodes/0x007E.md). Case 2 writes nothing: it
    /// re-executes every frame until the target's mount attachment reports
    /// ready, a signal no host of ours has, so it advances instead of spinning.
    fn emit_mount_cue(&mut self) {
        let case = self.byte_at(CHOCOBO_CASE_OFS);
        let target = ActorLookup(self.eventgetcode2(CHOCOBO_TARGET_OFS));
        let (status_event, mount_id) = if CHOCOBO_CASES_IDLE.contains(&case) {
            (STATUS_EVENT_IDLE, None)
        } else if CHOCOBO_CASES_CHOCOBO.contains(&case) {
            (STATUS_EVENT_CHOCOBO, None)
        } else if case == CHOCOBO_CASE_MOUNT {
            let id = self.getworkofs(CHOCOBO_MOUNT_ID_OFS, 0) as u16;
            (
                STATUS_EVENT_MOUNT,
                Some(id.wrapping_add(CHOCOBO_MOUNT_ID_BIAS)),
            )
        } else if case == CHOCOBO_CASE_UNMOUNT {
            (STATUS_EVENT_IDLE, Some(CHOCOBO_UNMOUNT_ID))
        } else {
            return;
        };
        self.cues.push(EventCue::Mount {
            target,
            status_event,
            mount_id,
        });
    }

    fn byte_at(&self, index: usize) -> u8 {
        self.event_data
            .get(self.exec_pointer + index)
            .copied()
            .unwrap_or(0)
    }

    /// `XiEvent::eventgetcode2`: little-endian u32 at `ExecPointer + index`
    /// (research/XiEvents/Event VM Functions.md). Out-of-bounds bytes read 0 and
    /// are counted like [`Self::eventgetcode`]'s.
    fn eventgetcode2(&self, index: usize) -> u32 {
        let at = self.exec_pointer + index;
        if at + 3 >= self.event_data.len() {
            self.count_oob_read(at);
        }
        u32::from_le_bytes([
            self.byte_at(index),
            self.byte_at(index + 1),
            self.byte_at(index + 2),
            self.byte_at(index + 3),
        ])
    }

    fn count_oob_read(&self, at: usize) {
        let seen = self.oob_reads.get();
        self.oob_reads.set(seen.saturating_add(1));
        if seen == 0 {
            tracing::debug!(
                at,
                bytecode_len = self.event_data.len(),
                "operand read past end of bytecode (yields 0; \
                 further out-of-bounds reads counted, not logged)"
            );
        }
    }

    /// `XiEvent::eventgetcode`: little-endian u16 at `ExecPointer + index`.
    /// Reads past the end of the bytecode yield 0 (retail reads unchecked
    /// memory; 0 is our deterministic stand-in) — they are counted in
    /// [`Self::oob_reads`] and the first one is logged (kuluu-zkuf).
    fn eventgetcode(&self, index: usize) -> u16 {
        let at = self.exec_pointer + index;
        if at + 1 >= self.event_data.len() {
            self.count_oob_read(at);
        }
        u16::from_le_bytes([self.byte_at(index), self.byte_at(index + 1)])
    }

    /// `XiEvent::getworkofs`: route a bytecode value to its backing store. Only
    /// the References and per-event `WorkLocal` stores are modeled; zone work
    /// arrays and entity/player accessors (0x7F00/0x7F80) return 0 until a host
    /// is wired (Stage 2). Returns a signed value (the VM treats work as `int`).
    fn getworkofs(&self, index: usize, shift: i32) -> i32 {
        let val = (self.eventgetcode(index) as i32).wrapping_add(shift) as u32;
        if let Some(value) = self.scene_operand(val) {
            return value;
        }
        if val & REFERENCE_FLAG != 0 {
            return self
                .references
                .get((val & REFERENCE_INDEX_MASK) as usize)
                .copied()
                .unwrap_or(0) as i32;
        }
        if val < 2048 {
            if val >= WORK_LOCAL_LEN as u32 {
                return 0;
            }
            return self.work_local[val as usize] as i32;
        }
        if (WORK_ZONE_BASE..WORK_ZONE_BASE + WORK_ZONE_LEN as u32).contains(&val) {
            return self.work_zone.lock().unwrap()[(val - WORK_ZONE_BASE) as usize] as i32;
        }
        0
    }

    /// `XiEvent::setworkofs`: write `value` to the store the bytecode value at
    /// `ExecPointer + index` selects. Mirrors [`getworkofs`](Self::getworkofs)'
    /// routing; References are read-only and the unmodeled zone/entity stores are
    /// no-ops.
    fn setworkofs(&mut self, index: usize, value: i32, shift: i32) {
        let val = (self.eventgetcode(index) as i32).wrapping_add(shift) as u32;
        if val & REFERENCE_FLAG != 0 {
            return;
        }
        if val < 2048 {
            if (val as usize) < WORK_LOCAL_LEN {
                self.work_local[val as usize] = value as u32;
            }
            return;
        }
        if (WORK_ZONE_BASE..WORK_ZONE_BASE + WORK_ZONE_LEN as u32).contains(&val) {
            let index = (val - WORK_ZONE_BASE) as usize;
            self.work_zone.lock().unwrap()[index] = value as u32;
            if (EVENT_PARAM_WORK_BASE..EVENT_PARAM_WORK_BASE + EVENT_PARAM_COUNT).contains(&index) {
                self.param_len = self.param_len.max(index - EVENT_PARAM_WORK_BASE + 1);
            }
        }
    }

    /// `XiEvent::getworkstrofs`: the WorkLocal string slot the bytecode operand
    /// at `index` selects (research/XiEvents/Event VM Functions.md). A
    /// References-flagged or out-of-range operand reads the global zero array
    /// retail returns — an empty string here.
    fn getworkstr(&self, index: usize) -> [u8; 16] {
        let val = self.eventgetcode(index) as u32;
        if val & REFERENCE_FLAG == 0 && (val as usize) < WORK_LOCAL_LEN {
            self.work_local_str[val as usize]
        } else {
            [0u8; 16]
        }
    }

    /// `XiEvent::setworkstrofs`: write `src` into the slot the operand at
    /// `index` selects; References are read-only and the write bound is
    /// [`WORK_STR_WRITE_LEN`], past which retail refuses the store.
    fn setworkstr(&mut self, index: usize, src: [u8; 16]) {
        let val = self.eventgetcode(index) as u32;
        if val & REFERENCE_FLAG != 0 {
            return;
        }
        if (val as usize) < WORK_STR_WRITE_LEN {
            self.work_local_str[val as usize] = src;
        }
    }

    /// Store `value` into the slot operand 1 selects and advance `width`, the
    /// shape every one-operand arithmetic opcode shares.
    fn store_unary(&mut self, value: i32, width: usize) {
        self.setworkofs(1, value, 0);
        self.exec_pointer += width;
    }

    /// Apply `f` to the slots operands 1 and 3 select, store into operand 1, and
    /// advance 5 — the shape every two-operand arithmetic opcode shares.
    fn store_binary(&mut self, f: impl Fn(i32, i32) -> i32) {
        let v1 = self.getworkofs(1, 0);
        let v2 = self.getworkofs(3, 0);
        self.store_unary(f(v1, v2), 5);
    }

    /// `XiEvent::CodeSETBITWORK` (0x40) / `CodeGETBITWORK` (0x41): build a
    /// contiguous bit mask spanning bit indices `[v1, v2]` and either store a
    /// masked, shifted value back (`set`) or extract one (`!set`). Used to pack
    /// the available dialog-menu option flags. Per
    /// research/XiEvents/OpCodes/0x0040.md,
    /// 0x0041.md — the mask is built by the same signed arithmetic-shift idiom.
    fn op_bitwork(&mut self, set: bool) {
        let v1 = self.getworkofs(1, 0);
        let v2 = self.getworkofs(3, 0);
        let mut mask: i32 = 0;
        for x in 0..32i32 {
            mask >>= 1;
            if v1 <= x && v2 >= x {
                mask |= i32::MIN;
            }
        }
        let shift = (v1 as u32) & 31;
        if set {
            let v3 = !mask & self.getworkofs(5, 0);
            let v4 = self.getworkofs(7, 0);
            self.setworkofs(5, v3 | (mask & v4.wrapping_shl(shift)), 0);
        } else {
            let v3 = self.getworkofs(5, 0);
            self.setworkofs(7, (mask & v3).wrapping_shr(shift), 0);
        }
    }

    /// `XiEvent::CodeBITTEST` (0x003E): branch on one bit of a work slot. The
    /// bit index picks both the word (`>> 5`, applied as `getworkofs`' index
    /// shift) and the bit within it (research/XiEvents/OpCodes/0x003E.md).
    fn op_bit_test(&mut self, op: u8) {
        let bit = self.getworkofs(BIT_TEST_INDEX_OFS, 0);
        let word = self.getworkofs(BIT_TEST_WORD_OFS, bit >> BIT_TEST_WORD_SHIFT);
        if word & (1i32 << (bit & BIT_TEST_BIT_MASK)) != 0 {
            self.exec_pointer += OPCODE_META[op as usize].size as usize;
        } else {
            self.exec_pointer = self.eventgetcode(BIT_TEST_TARGET_OFS) as usize;
        }
    }

    /// `XiEvent::CodeIF` (0x0002): conditional branch with 11 comparison kinds.
    /// The taken-branch target at +6 is an absolute offset into EventData, like
    /// GOTO/JUMP: retail assigns `ExecPointer = FUNC_XiEvent_eventgetcode(this, 6)`
    /// (research/XiEvents/OpCodes/0x0002.md pseudo-code; 0x0001.md, 0x001A.md).
    /// The case tables in the same doc print `ExecPointer += val3`; retail
    /// bytecode refutes that reading (ffxi-event/examples/zz-jump-check.rs: across
    /// zones 77, 234 and 241 every IF target lands on an instruction boundary only
    /// when read absolute).
    fn op_if(&mut self) {
        let kind = self.byte_at(5) & IF_KIND_MASK;
        let target = self.eventgetcode(6) as usize;
        let v1 = self.getworkofs(1, 0);
        let v2 = self.getworkofs(3, 0);
        let take = match kind {
            0 => v1 != v2, // case 0 falls through on equal (jump on NOT equal)
            1 | 7 => v1 == v2,
            2 => v1 <= v2,
            3 => v1 >= v2,
            4 => v1 < v2,
            5 => v1 > v2,
            6 | 9 => (v2 as u32 & v1 as u32) == 0,
            8 => (v1 as u32 | v2 as u32) == 0,
            10 => (!(v1 as u32) & v2 as u32) == 0,
            _ => true,
        };
        self.exec_pointer = if take { target } else { self.exec_pointer + 8 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::event_dat::EventBlock;

    /// Build a one-event block: `event_data` bytecode entered at offset 0.
    fn block(event_data: Vec<u8>, references: Vec<u32>) -> EventBlock {
        EventBlock {
            actor: ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            event_ids: vec![7],
            event_offsets: vec![0],
            references,
            event_data,
        }
    }

    fn vm(event_data: Vec<u8>, references: Vec<u32>) -> EventVm {
        EventVm::start(&block(event_data, references), 7, 5, vec![]).unwrap()
    }

    #[test]
    fn empty_actor_entry_uses_only_an_unambiguous_executable_participant() {
        use ffxi_dat::event_dat::{EventBlockSource, EventDat};
        let mut empty = block(vec![OP_END], vec![]);
        empty.actor = 1;
        let mut driver = block(vec![OP_MESSAGE, 0, 0x80, OP_END], vec![0]);
        driver.actor = 2;
        let mut dat = EventDat {
            blocks: vec![empty, driver.clone()],
        };
        let (chosen, source) = EventVm::driving_block(&dat, 1, 7).unwrap();
        assert_eq!(chosen.actor, 2);
        assert_eq!(source, EventBlockSource::SoleOwnerElsewhere);
        driver.actor = 3;
        dat.blocks.push(driver);
        assert_eq!(EventVm::driving_block(&dat, 1, 7).unwrap().0.actor, 1);
        dat.blocks.last_mut().unwrap().event_data.clear();
        assert_eq!(EventVm::driving_block(&dat, 1, 7).unwrap().0.actor, 1);
    }

    #[test]
    fn owner_blocks_lists_every_non_end_exact_owner() {
        let master = block(vec![OP_END], vec![]);
        let mut owner_a = block(vec![OP_EVENTHIDE, 1, 0, 0, 0, 0, OP_END], vec![]);
        owner_a.actor = NPC_SERVER_ID;
        let mut owner_b = block(vec![OP_END, OP_EVENTHIDE, 1, 0, 0, 0, 0, OP_END], vec![]);
        owner_b.actor = NPC_SERVER_ID + 1;
        owner_b.event_ids = vec![5, 7];
        owner_b.event_offsets = vec![0, 1];
        let mut wildcard_only = block(vec![OP_END], vec![]);
        wildcard_only.actor = NPC_SERVER_ID + 2;
        wildcard_only.event_ids = vec![ffxi_dat::event_dat::EVENT_ID_WILDCARD];
        let mut end_exact = block(vec![OP_END], vec![]);
        end_exact.actor = NPC_SERVER_ID + 3;
        let dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![master, owner_a, owner_b, wildcard_only, end_exact],
        };

        let owners = EventVm::owner_blocks(&dat, 7);
        assert_eq!(
            owners.iter().map(|(b, _)| b.actor).collect::<Vec<_>>(),
            [NPC_SERVER_ID, NPC_SERVER_ID + 1]
        );
        assert_eq!(
            owners.iter().map(|(_, entry)| *entry).collect::<Vec<_>>(),
            [0, 1],
            "entries are positional within each owner's own bytecode"
        );
    }

    #[test]
    fn spawn_owner_runs_owner_blocks_in_parallel_from_event_start() {
        // The master's program is just END, like 531's zone block: the owners'
        // programs are what the event plays. Owner A hides and ends; owner B
        // parks on a one-second wait, so the master's END must stay Waiting
        // until B drains (research/XiEvents/Event VM Functions.md
        // InitEvent2/XiEventInit).
        const ONE_SECOND: u32 = WAIT_UNITS_PER_SEC as u32;
        let mut hide = vec![OP_EVENTHIDE, 1];
        hide.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        hide.push(OP_END);
        let mut owner_a = block(hide, vec![]);
        owner_a.actor = NPC_SERVER_ID;
        let mut owner_b = block(vec![OP_WAIT, REF0[0], REF0[1], OP_END], vec![ONE_SECOND]);
        owner_b.actor = NPC_SERVER_ID + 1;
        let dat = std::sync::Arc::new(ffxi_dat::event_dat::EventDat {
            blocks: vec![block(vec![OP_END], vec![]), owner_a, owner_b],
        });

        let mut e = vm(vec![OP_END], vec![]);
        e.attach_scene(
            dat.clone(),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        for (owner, entry) in EventVm::owner_blocks(&dat, 7) {
            if owner.actor != ffxi_dat::event_dat::ZONE_PLAYER_ACTOR {
                e.spawn_owner(owner, entry);
            }
        }

        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master's END must not end the event while an owner is running"
        );
        e.tick(0.5);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "halfway through owner B's wait"
        );
        e.tick(0.6);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the event ends only when every owner block has drained"
        );
        assert_eq!(
            e.take_cues(),
            [EventCue::ActorHide {
                target: ActorLookup(NPC_SERVER_ID),
                hide: true,
            }],
            "owner A's cue bubbles up with owner A as the event entity"
        );
    }

    /// The onEventUpdate round trip on an owner child: the child holds the
    /// pending tag (0x43 SENDTAG), the master's pending_tag sees it — the
    /// session's EventRecvPending gate keys on it — and the master's
    /// ack_server releases the child (retail's RecPendingFlag is one global
    /// shared by every entity VM, research/XiEvents/OpCodes/0x0043.md).
    #[test]
    fn owner_childs_pending_tag_is_visible_and_released_through_the_master() {
        let mut owner_a = block(vec![OP_SENDTAG, 0, OP_SENDTAG, 1, OP_END], vec![]);
        owner_a.actor = NPC_SERVER_ID;
        let dat = std::sync::Arc::new(ffxi_dat::event_dat::EventDat {
            blocks: vec![block(vec![OP_END], vec![]), owner_a],
        });

        let mut e = vm(vec![OP_END], vec![]);
        e.attach_scene(
            dat.clone(),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        for (owner, entry) in EventVm::owner_blocks(&dat, 7) {
            if owner.actor != ffxi_dat::event_dat::ZONE_PLAYER_ACTOR {
                e.spawn_owner(owner, entry);
            }
        }

        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the owner parks on its 0x43 poll"
        );
        assert!(
            e.pending_tag().is_some(),
            "the master must see the child's held tag"
        );
        e.ack_server();
        assert!(
            e.pending_tag().is_none(),
            "the release must reach the child"
        );
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the owner runs past the poll to END and the event ends"
        );
    }

    /// s2c PENDINGNUM's Work_Zone slots drive the owner child's 0x47: the
    /// child's onEventUpdate loop reads the slots its sibling state wrote
    /// (Work_Zone is one shared global array, research/XiEvents/Event VM
    /// Functions.md getworkofs/setworkofs), and the whole round trip — tag,
    /// PENDINGNUM, position ack — drains to Done.
    #[test]
    fn pending_num_reaches_the_owner_childs_work_zone() {
        let mut owner = vec![OP_SENDTAG, 0, OP_SENDTAG, 1];
        owner.extend_from_slice(&[
            OP_EVENTPOSSET,
            0,
            0x02,
            0x10,
            0x03,
            0x10,
            0x04,
            0x10,
            0x00,
            0x00,
            OP_EVENTPOSSET,
            1,
        ]);
        owner.push(OP_END);
        let mut owner_b = block(owner, vec![]);
        owner_b.actor = NPC_SERVER_ID;
        let dat = std::sync::Arc::new(ffxi_dat::event_dat::EventDat {
            blocks: vec![block(vec![OP_END], vec![]), owner_b],
        });

        let mut e = vm(vec![OP_END], vec![]);
        e.attach_scene(
            dat.clone(),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        for (owner, entry) in EventVm::owner_blocks(&dat, 7) {
            if owner.actor != ffxi_dat::event_dat::ZONE_PLAYER_ACTOR {
                e.spawn_owner(owner, entry);
            }
        }

        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the owner parks on its 0x43 poll"
        );
        e.apply_pending_num(&[11, 22, 33, 44, 55, 66, 77, 88]);
        e.ack_server();
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the owner's 0x47 case 0 holds on the position round trip"
        );
        assert_eq!(
            e.take_scene_actions(),
            [crate::vm::scene::SceneAction::PositionUpdate {
                position: crate::vm::scene::EventPosition {
                    x: 11,
                    y: 33,
                    z: 22,
                    heading: 0,
                },
                end_para: 0,
            }],
            "the 0x47 must read the PENDINGNUM slots the child's copy holds"
        );
        e.acknowledge_position(crate::vm::scene::EventPosition {
            x: 11,
            y: 33,
            z: 22,
            heading: 0,
        });
        e.acknowledge_event();
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the position ack releases the 0x47 and the event ends"
        );
    }

    #[test]
    fn owner_child_setworkofs_lands_in_master_and_siblings() {
        const NPC2: u32 = 0x0100_02C6;
        const ONE_SECOND: u32 = WAIT_UNITS_PER_SEC as u32;
        let owner_a = vec![
            OP_GET_STORE,
            0x03,
            0x10,
            0x00,
            0x80,
            OP_WAIT,
            0x01,
            0x80,
            OP_END,
        ];
        let owner_b = vec![
            OP_GET_STORE,
            0x00,
            0x00,
            0x03,
            0x10,
            OP_WAIT,
            0x01,
            0x80,
            OP_END,
        ];
        let mut block_a = block(owner_a, vec![0xDEAD, ONE_SECOND]);
        block_a.actor = NPC_SERVER_ID;
        let mut block_b = block(owner_b, vec![0, ONE_SECOND]);
        block_b.actor = NPC2;
        let dat = std::sync::Arc::new(ffxi_dat::event_dat::EventDat {
            blocks: vec![block(vec![OP_END], vec![]), block_a, block_b],
        });

        let mut e = vm(vec![OP_END], vec![]);
        e.attach_scene(
            dat.clone(),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        for (owner, entry) in EventVm::owner_blocks(&dat, 7) {
            if owner.actor != ffxi_dat::event_dat::ZONE_PLAYER_ACTOR {
                e.spawn_owner(owner, entry);
            }
        }

        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "both owners park on their waits"
        );
        assert_eq!(
            e.work_zone.lock().unwrap()[3],
            0xDEAD,
            "the master sees owner A's write"
        );
        assert_eq!(
            e.child_mut(NPC_SERVER_ID, 0).work_zone.lock().unwrap()[3],
            0xDEAD,
            "owner A sees its own write"
        );
        assert_eq!(
            e.child_mut(NPC2, 0).work_zone.lock().unwrap()[3],
            0xDEAD,
            "owner B sees owner A's write"
        );
        assert_eq!(
            e.child_mut(NPC2, 0).work_local[0],
            0xDEAD,
            "owner B's read went through the shared cell"
        );
    }

    #[test]
    fn trigger_params_seed_all_eight_work_slots_without_overwriting_results() {
        let params = vec![1_300_000, 100, -1, i32::MIN, i32::MAX, 17, 29, 41];
        let event = EventVm::start(&block(vec![OP_END], vec![]), 7, 5, params.clone()).unwrap();
        assert_eq!(event.work_zone(0), 0);
        assert_eq!(event.work_zone(1), 0);
        for (index, expected) in params.iter().enumerate() {
            assert_eq!(event.work_zone(index + EVENT_PARAM_WORK_BASE), *expected);
        }
        assert_eq!(
            event.work_zone(EVENT_PARAM_WORK_BASE + EVENT_PARAM_COUNT),
            0
        );
    }

    /// Operand selecting `work_zone[0]`, little-endian.
    const DST: [u8; 2] = [0x00, 0x10];
    /// Operand selecting `references[1]`, little-endian.
    const SRC: [u8; 2] = [0x01, 0x80];
    /// Filler byte for program regions a taken branch skips over.
    const POISON: u8 = 255;

    /// `work_zone[0] = references[0]`, so a test can seed the destination the
    /// arithmetic opcodes read-modify-write.
    fn seed() -> [u8; 5] {
        [OP_GET_STORE, DST[0], DST[1], 0x00, 0x80]
    }

    /// The two-operand work-slot arithmetic family, against the pseudo-code
    /// bodies in research/XiEvents/OpCodes/. These touch no host state, so a
    /// wrong result is a wrong branch in every event that loops on one.
    #[test]
    fn two_operand_arithmetic_matches_retail() {
        for (op, a, b, want) in [
            (OP_ADD, 7i32, 5i32, 12i32),
            (OP_SUB, 7, 5, 2),
            (OP_MUL, 7, 5, 35),
            (OP_DIV, 30, 5, 6),
            (OP_AND, 0b1100, 0b1010, 0b1000),
            (OP_OR, 0b1100, 0b1010, 0b1110),
            (OP_XOR, 0b1100, 0b1010, 0b0110),
            (OP_SHL, 1, 4, 16),
            (OP_SHR, 16, 4, 1),
            (OP_BIT_SET, 0b0001, 2, 0b0101),
            (OP_BIT_CLEAR, 0b0101, 2, 0b0001),
        ] {
            let mut data = seed().to_vec();
            data.extend_from_slice(&[op, DST[0], DST[1], SRC[0], SRC[1], OP_END]);
            let mut e = vm(data, vec![a as u32, b as u32]);

            assert_eq!(e.step(), StepResult::Done, "op 0x{op:02X} must reach END");
            assert_eq!(e.work_zone(0), want, "op 0x{op:02X}({a}, {b})");
        }
    }

    /// `OP_DIV` guards on *both* operands, so a zero numerator or a zero
    /// denominator stores 0 rather than dividing
    #[test]
    fn divide_by_zero_and_of_zero_both_store_zero() {
        for (a, b) in [(0i32, 5i32), (30, 0)] {
            let mut data = seed().to_vec();
            data.extend_from_slice(&[OP_DIV, DST[0], DST[1], SRC[0], SRC[1], OP_END]);
            let mut e = vm(data, vec![a as u32, b as u32]);

            assert_eq!(e.step(), StepResult::Done);
            assert_eq!(e.work_zone(0), 0, "{a} / {b}");
        }
    }

    /// The one-operand family. `OP_INC` is the loop counter that, while it was
    /// only being skipped by width, left corpus events spinning until the
    /// opcode budget killed them.
    #[test]
    fn one_operand_arithmetic_matches_retail() {
        for (op, seed_val, want) in [
            (OP_SET_ONE, 9i32, 1i32),
            (OP_SET_ZERO, 9, 0),
            (OP_INC, 9, 10),
            (OP_DEC, 9, 8),
        ] {
            let mut data = seed().to_vec();
            data.extend_from_slice(&[op, DST[0], DST[1], OP_END]);
            let mut e = vm(data, vec![seed_val as u32]);

            assert_eq!(e.step(), StepResult::Done, "op 0x{op:02X} must reach END");
            assert_eq!(e.work_zone(0), want, "op 0x{op:02X}({seed_val})");
        }
    }

    /// `OP_SWAP` swaps the two slots rather than storing a computed value.
    #[test]
    fn endian_swap_exchanges_both_slots() {
        let data = vec![OP_SWAP, DST[0], DST[1], 0x01, 0x10, OP_END];
        let mut e = vm(data, vec![]);
        e.work_zone.lock().unwrap()[0] = 11;
        e.work_zone.lock().unwrap()[1] = 22;

        assert_eq!(e.step(), StepResult::Done);
        assert_eq!((e.work_zone(0), e.work_zone(1)), (22, 11));
    }

    /// `OP_BITARRAY_SET` addresses a bit array: operand 3 is the flat bit
    /// index, so `>> 5` picks the slot and `& 0x1F` the bit within it, and
    /// operand 5 bounds the array. The fixture drives bit 33 (slot 1, bit 1);
    /// bound 2 admits it, bound 1 does not
    /// (research/XiEvents/OpCodes/0x003C.md).
    #[test]
    fn bitarray_set_addresses_slot_and_bit() {
        for (bound, want_slot1) in [(2i32, 0b10i32), (1, 0)] {
            let data = vec![
                OP_BITARRAY_SET,
                DST[0],
                DST[1],
                0x00,
                0x80,
                0x01,
                0x80,
                OP_END,
            ];
            let mut e = vm(data, vec![33, bound as u32]);

            assert_eq!(e.step(), StepResult::Done);
            assert_eq!(e.work_zone(1), want_slot1, "bound {bound}");
            assert_eq!(e.work_zone(0), 0, "bound {bound} must not touch slot 0");
        }
    }

    /// One authored second of wait must cost a second of host clock. Before the
    /// VM had one, a whole scene's worth of cues landed in the tick the player
    /// answered and every fade snapped. The `OP_WAIT` duration goes through
    /// `getworkofs` like any operand, so it rides a Reference here.
    #[test]
    fn a_timed_wait_spends_the_time_it_authors() {
        const ONE_SECOND: u32 = WAIT_UNITS_PER_SEC as u32;
        let mut e = vm(vec![OP_WAIT, 0x00, 0x80, OP_END], vec![ONE_SECOND]);

        assert_eq!(e.step(), StepResult::Waiting);
        e.tick(0.5);
        assert_eq!(e.step(), StepResult::Waiting, "half way is still waiting");
        e.tick(0.6);
        assert_eq!(e.step(), StepResult::Done);
    }

    /// Retail re-reads `WaitTime` only when it is not already counting, so a
    /// stepped-but-unexpired wait must not restart and strand the scene.
    #[test]
    fn stepping_a_held_wait_does_not_restart_it() {
        let mut e = vm(
            vec![OP_WAIT, 0x00, 0x80, OP_END],
            vec![WAIT_UNITS_PER_SEC as u32],
        );
        assert_eq!(e.step(), StepResult::Waiting);
        for _ in 0..10 {
            e.tick(0.09);
            assert_eq!(e.step(), StepResult::Waiting);
        }
        e.tick(0.2);
        assert_eq!(e.step(), StepResult::Done, "the clock accumulated");
    }

    /// A zero-length wait still yields once — retail sets RetFlag before
    /// testing the timer — so the tick cost applies at zero length too.
    /// Expiry is strictly `< 0.0` as in retail, so clearing takes a real
    /// (nonzero) slice of host clock, not the instant it was armed.
    #[test]
    fn a_zero_length_wait_still_costs_a_tick() {
        let mut e = vm(vec![OP_WAIT, 0x00, 0x80, OP_END], vec![0]);
        assert_eq!(e.step(), StepResult::Waiting);
        e.tick(0.0);
        assert_eq!(e.step(), StepResult::Waiting, "no host clock has passed");
        e.tick(0.01);
        assert_eq!(e.step(), StepResult::Done);
    }

    #[test]
    fn end_opcode_finishes() {
        let mut e = vm(vec![OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.step(), StepResult::Done);
        assert!(!e.ran_past_end(), "END is a normal finish");
        assert_eq!(e.oob_reads(), 0);
    }

    /// A non-jumping, non-yield opcode (size 1) then off the end.
    #[test]
    fn running_off_the_end_is_done_and_flagged() {
        let mut e = vm(vec![OP_CANCEL_DISARM], vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert!(e.ran_past_end(), "no END opcode was executed");
    }

    #[test]
    fn oob_operand_read_is_counted_and_yields_zero() {
        // GOTO's u16 target has only its low byte in the data; the high byte
        // is past the end and reads as 0, so the jump lands at 2 — which is
        // itself past the end, finishing the event.
        let mut e = vm(vec![OP_GOTO, 2], vec![]);
        assert_eq!(e.oob_reads(), 0);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.oob_reads(), 1);
        assert_eq!(e.exec_pointer(), 2);
        assert!(e.ran_past_end());
    }

    #[test]
    fn goto_then_end() {
        // 0x01 jumps to offset 4 (the END), skipping a bogus byte at 3.
        let mut e = vm(vec![OP_GOTO, 4, 0, 0xFF, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 4);
    }

    #[test]
    fn reqset_family_skips_by_size_and_continues() {
        for (op, size) in [
            (OP_REQSET, 7usize),
            (OP_REQSET_CHECKED, 7),
            (OP_REQSET_PRIORITY, 7),
            (OP_REQWAIT, 6),
        ] {
            assert_eq!(
                OPCODE_META[op as usize].size as usize, size,
                "op 0x{op:02X} size drifted from research/XiEvents/OpCodes"
            );
            let mut data = vec![op];
            data.extend(std::iter::repeat_n(0u8, size - 1));
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            assert_eq!(
                e.step(),
                StepResult::Done,
                "op 0x{op:02X} should run to END"
            );
            assert_eq!(e.exec_pointer(), size, "op 0x{op:02X} advanced wrong size");
        }
    }

    /// An empty key emits no cue but still advances the full width; a real
    /// key's cue is pinned by the dedicated tests below.
    #[test]
    fn loadextscheduler_family_advances_by_size_with_an_empty_key() {
        for op in [OP_LOADEXTSCHEDULER, OP_LOADEXTSCHEDULER2] {
            let size = OPCODE_META[op as usize].size as usize;
            assert_eq!(
                size, 15,
                "op 0x{op:02X} size drifted from research/XiEvents/OpCodes (param3=0 advance)"
            );
            let mut data = vec![op];
            data.extend(std::iter::repeat_n(0u8, size - 1));
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            assert_eq!(
                e.step(),
                StepResult::Done,
                "op 0x{op:02X} should run to END"
            );
            assert_eq!(e.exec_pointer(), size, "op 0x{op:02X} advanced wrong size");
        }
    }

    /// The bare test VM's event entity has no Type data, so an accepted Type
    /// is installed under the vm() helper's speaker index (5); the
    /// `OP_LOADEXTSCHEDULER` gate loads only for entity Type {1,2,7,8}.
    #[test]
    fn loadextscheduler_emits_ext_cue_and_advances_15() {
        /// Band 0 of the file operand: the event-motion base the VM adds.
        const FILE_OPERAND: u32 = 5;
        let mut o = REF0.to_vec();
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(b"abcd");
        let mut types = std::collections::HashMap::new();
        types.insert(5u32, 2u8);
        assert_eq!(
            cues_of_with_types(OP_LOADEXTSCHEDULER, &o, vec![FILE_OPERAND], &types),
            [EventCue::ExtScheduler {
                motion: Some(ExtSchedulerMotion::Event(event_motion_dat_id(
                    FILE_OPERAND as i32
                ))),
                actor1: ActorLookup::EVENT_ENTITY,
                actor2: ActorLookup::EVENT_ENTITY,
                key: *b"abcd",
            }]
        );
    }

    #[test]
    fn loadextscheduler2_maps_the_tpc_package_to_its_band_ids() {
        /// The Sandy scene's Tpc package.
        const PACKAGE: u32 = 20;
        let mut o = REF0.to_vec();
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(b"abcd");
        assert_eq!(
            cues_of(OP_LOADEXTSCHEDULER2, &o, vec![PACKAGE]),
            [EventCue::ExtScheduler {
                motion: Some(ExtSchedulerMotion::Tpc(TpcMotionPackages {
                    a: 32_732,
                    b_set: 32_802,
                    b_clear: 32_872,
                })),
                actor1: ActorLookup::EVENT_ENTITY,
                actor2: ActorLookup::EVENT_ENTITY,
                key: *b"abcd",
            }]
        );
    }

    #[test]
    fn loadextscheduler2_out_of_range_package_carries_no_motion() {
        /// A package at the range limit: the gate loads nothing.
        const PACKAGE: u32 = TPC_PACKAGE_OUT_OF_RANGE;
        let mut o = REF0.to_vec();
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(b"abcd");
        assert_eq!(
            cues_of(OP_LOADEXTSCHEDULER2, &o, vec![PACKAGE]),
            [EventCue::ExtScheduler {
                motion: None,
                actor1: ActorLookup::EVENT_ENTITY,
                actor2: ActorLookup::EVENT_ENTITY,
                key: *b"abcd",
            }]
        );
    }

    #[test]
    fn loadextscheduler_with_xxxx_key_emits_no_cue() {
        let mut o = REF0.to_vec();
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        for key in [b"xxxx", b"\0\0\0\0"] {
            let mut ops = o.clone();
            ops.extend_from_slice(key);
            assert!(
                cues_of(OP_LOADEXTSCHEDULER, &ops, vec![5]).is_empty(),
                "key {key:?} must not stage a motion"
            );
        }
    }

    /// The `OP_LOADEXTSCHEDULER` gate: ReadEventMotionRes loads only for entity
    /// Type {1,2,7,8}. One accepted and one refused Type, pinned on the cue and
    /// on the same-batch hold a following `OP_WAITSCHEDULOR` parks on; the
    /// Types install under the vm() helper's speaker index.
    #[test]
    fn loadextscheduler_gates_on_the_entity_type() {
        const FILE_OPERAND: u32 = 5;
        let mut o = REF0.to_vec();
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(b"abcd");
        let types = |t: u8| {
            let mut m = std::collections::HashMap::new();
            m.insert(5u32, t);
            m
        };
        assert_eq!(
            cues_of_with_types(OP_LOADEXTSCHEDULER, &o, vec![FILE_OPERAND], &types(2)),
            [EventCue::ExtScheduler {
                motion: Some(ExtSchedulerMotion::Event(event_motion_dat_id(
                    FILE_OPERAND as i32
                ))),
                actor1: ActorLookup::EVENT_ENTITY,
                actor2: ActorLookup::EVENT_ENTITY,
                key: *b"abcd",
            }]
        );
        assert!(
            wait_after_loader_parks(OP_LOADEXTSCHEDULER, &o, &types(2)),
            "an accepted load must arm the same-batch hold"
        );
        assert!(
            cues_of_with_types(OP_LOADEXTSCHEDULER, &o, vec![FILE_OPERAND], &types(0)).is_empty()
        );
        assert!(
            !wait_after_loader_parks(OP_LOADEXTSCHEDULER, &o, &types(0)),
            "a refused load must arm no hold"
        );
    }

    /// The `OP_LOADEXTSCHEDULER2` gate: ReadTpcEventMotionRes loads only for
    /// entity Type {0,1,6}; the Types install under the vm() helper's speaker
    /// index.
    #[test]
    fn loadextscheduler2_gates_on_the_entity_type() {
        const PACKAGE: u32 = 20;
        let mut o = REF0.to_vec();
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(b"abcd");
        let types = |t: u8| {
            let mut m = std::collections::HashMap::new();
            m.insert(5u32, t);
            m
        };
        assert_eq!(
            cues_of_with_types(OP_LOADEXTSCHEDULER2, &o, vec![PACKAGE], &types(6)),
            [EventCue::ExtScheduler {
                motion: Some(ExtSchedulerMotion::Tpc(TpcMotionPackages {
                    a: 32_732,
                    b_set: 32_802,
                    b_clear: 32_872,
                })),
                actor1: ActorLookup::EVENT_ENTITY,
                actor2: ActorLookup::EVENT_ENTITY,
                key: *b"abcd",
            }]
        );
        assert!(
            wait_after_loader_parks(OP_LOADEXTSCHEDULER2, &o, &types(6)),
            "an accepted load must arm the same-batch hold"
        );
        assert!(cues_of_with_types(OP_LOADEXTSCHEDULER2, &o, vec![PACKAGE], &types(2)).is_empty());
        assert!(
            !wait_after_loader_parks(OP_LOADEXTSCHEDULER2, &o, &types(2)),
            "a refused load must arm no hold"
        );
    }

    /// GOTO to its own offset: the tightest loop the bytecode can express.
    /// GOTO is implemented, so this must report as a spin, not as work to do,
    /// and it stays stopped on the next tick.
    #[test]
    fn a_script_that_loops_forever_stops_instead_of_hanging() {
        let mut e = vm(vec![OP_GOTO, 0x00, 0x00], vec![]);
        assert_eq!(e.step(), StepResult::Spun(OP_GOTO));
        assert_eq!(e.step(), StepResult::Done);
    }

    /// Sizes are load-bearing: a wrong width lands mid-instruction. With no
    /// host-armed hold the wait advances immediately; the hold path is pinned
    /// by the dedicated tests below.
    #[test]
    fn scheduler_wait_family_falls_through_without_a_hold() {
        for (op, size) in [
            (OP_WAITSCHEDULOR, 13usize),
            (OP_WAITMAPSCHEDULOR, 13),
            (OP_WAITLOADSCHEDULER, 15),
        ] {
            assert_eq!(
                OPCODE_META[op as usize].size as usize, size,
                "op 0x{op:02X} size drifted from research/XiEvents/OpCodes"
            );
            let mut data = vec![op];
            data.extend(std::iter::repeat_n(0u8, size - 1));
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            assert_eq!(
                e.step(),
                StepResult::Done,
                "op 0x{op:02X} should run to END"
            );
            assert_eq!(e.exec_pointer(), size, "op 0x{op:02X} advanced wrong size");
        }
    }

    #[test]
    fn waitloadsched_twin_falls_through_without_a_hold() {
        for op in [
            OP_WAITLOADSCHED_TWIN_A0,
            OP_WAITLOADSCHED_TWIN_BC,
            OP_WAITLOADSCHED_TWIN_C6,
            OP_WAITLOADSCHED_TWIN_CE,
            OP_WAITLOADSCHED_TWIN_D1,
            OP_WAITLOADSCHED_TWIN_D6,
        ] {
            assert_eq!(
                OPCODE_META[op as usize].size as usize, 15,
                "op 0x{op:02X} size drifted from research/XiEvents/OpCodes"
            );
            let mut data = vec![op];
            data.extend(std::iter::repeat_n(0u8, 14));
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            assert_eq!(
                e.step(),
                StepResult::Done,
                "op 0x{op:02X} should run to END"
            );
            assert_eq!(e.exec_pointer(), 15, "op 0x{op:02X} advanced wrong size");
        }
    }

    #[test]
    fn waitloadsched_twin_parks_on_the_actors_hold() {
        /// A literal server id with no References entry: it resolves to itself.
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        for op in [OP_WAITLOADSCHED_TWIN_A0, OP_WAITLOADSCHED_TWIN_D1] {
            let mut data = vec![op];
            data.extend(std::iter::repeat_n(0u8, 2));
            data.extend_from_slice(&ACTOR.to_le_bytes());
            data.extend_from_slice(&0u32.to_le_bytes());
            data.extend_from_slice(&key);
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC);
            assert_eq!(
                e.step(),
                StepResult::Waiting,
                "op 0x{op:02X} parks on its hold"
            );
            e.tick(1.1);
            assert_eq!(
                e.step(),
                StepResult::Done,
                "op 0x{op:02X} falls through after expiry"
            );
            assert_eq!(e.exec_pointer(), 15);
        }
    }

    #[test]
    fn actor_nop_yields_a_frame_then_advances() {
        let mut data = vec![OP_ACTOR_NOP];
        data.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the deprecated yield holds the frame"
        );
        e.tick(1.0 / 60.0);
        assert_eq!(e.step(), StepResult::Done, "the next frame runs past it");
        assert_eq!(e.exec_pointer(), 5);
        assert!(e.take_cues().is_empty());
    }

    /// 0x98 yields while the zone is reading data; this VM starts no zone
    /// read, so it takes the one-byte advance
    /// (research/XiEvents/OpCodes/0x0098.md).
    #[test]
    fn zone_read_yield_advances_past_itself() {
        let mut e = vm(vec![OP_ZONE_READ_YIELD, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 1);
    }

    #[test]
    fn anim_yield_parks_while_the_event_entity_animates() {
        let key: [u8; 4] = *b"abcd";
        let program = || vm(vec![OP_ANIM_YIELD, OP_END], vec![]);
        let mut e = program();
        assert_eq!(
            e.step(),
            StepResult::Done,
            "no animation: the yield falls through"
        );
        assert_eq!(e.exec_pointer(), 1);
        let mut e = program();
        e.hold_action(ActorLookup::EVENT_ENTITY, key, WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "a running animation parks the yield"
        );
        e.tick(1.1);
        assert_eq!(e.step(), StepResult::Done, "an expired hold falls through");
    }

    /// A 0x9B after a 0x6E on the event entity in one pass parks on the
    /// same-batch start, the way retail's AnimationPlay goes up at the emote
    /// (research/XiEvents/OpCodes/0x009B.md).
    #[test]
    fn anim_yield_parks_on_the_same_pass_emote() {
        let mut data = vec![OP_EMOT];
        data.extend_from_slice(&ActorLookup::EVENT_ENTITY.0.to_le_bytes());
        data.extend_from_slice(&REF0);
        data.push(OP_ANIM_YIELD);
        data.push(OP_END);
        let mut e = vm(data, vec![7]);
        assert_eq!(e.step(), StepResult::Waiting);
    }

    #[test]
    fn yield_forever_parks_without_advancing() {
        let mut e = vm(vec![OP_YIELD_FOREVER, OP_END], vec![]);
        for frame in 0..3 {
            assert_eq!(
                e.step(),
                StepResult::Waiting,
                "frame {frame}: the deprecated yield never advances"
            );
            assert_eq!(e.exec_pointer(), 0, "frame {frame}: the pointer holds");
            e.tick(1.0 / 60.0);
        }
    }

    /// The liveness classification: one program per Park variant, the input
    /// the host's stall check reads between steps.
    #[test]
    fn park_classifies_each_yield_state() {
        let mut e = vm(vec![OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.park(), Park::None, "an ended VM is not parked");

        let mut e = vm(vec![OP_MESSAGE, 0x00, 0x80, OP_MESWAIT, OP_END], vec![100]);
        assert!(matches!(e.step(), StepResult::AwaitMessage(_)));
        assert_eq!(
            e.park(),
            Park::Frame,
            "a displayed frame waits on the player"
        );

        let mut e = vm(vec![OP_WAIT, 0x28, 0x01, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Waiting);
        assert_eq!(e.park(), Park::TimedWait, "a timed wait has units left");

        let mut e = vm(
            vec![
                OP_WAITSCHEDULOR,
                0xF8,
                0xFF,
                0xFF,
                0x7F,
                0,
                0,
                0,
                0,
                b'm',
                b'a',
                b'i',
                b'n',
                OP_END,
            ],
            vec![],
        );
        e.hold_action_pending(ActorLookup::EVENT_ENTITY, *b"main");
        assert_eq!(e.step(), StepResult::Waiting);
        assert_eq!(
            e.park(),
            Park::Hold,
            "a pending hold is the host's to release"
        );

        // Case 0 sends the tag and runs into its case-1 poll, which yields
        // until the s2c ack (research/XiEvents/OpCodes/0x0043.md).
        let mut e = vm(vec![OP_SENDTAG, 0x00, OP_SENDTAG, 0x01, OP_END], vec![]);
        assert!(matches!(e.step(), StepResult::AwaitServerAck(_)));
        assert_eq!(
            e.park(),
            Park::ServerAck,
            "a pending tag waits on the s2c ack"
        );

        let mut e = vm(vec![OP_YIELD_FOREVER, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Waiting);
        assert_eq!(
            e.park(),
            Park::Dead,
            "a yield with nothing armed moves on no clock"
        );
    }

    /// force_cancel ends the event from the host side: a VM parked on each
    /// Park variant reports Cancelled from the next step, and reads as
    /// un-parked after.
    #[test]
    fn force_cancel_reports_cancelled_from_every_park() {
        let programs = [
            (vec![OP_END], vec![]),
            (vec![OP_MESSAGE, 0x00, 0x80, OP_MESWAIT, OP_END], vec![100]),
            (vec![OP_WAIT, 0x28, 0x01, OP_END], vec![]),
            (vec![OP_YIELD_FOREVER, OP_END], vec![]),
            (vec![OP_SENDTAG, 0x00, OP_SENDTAG, 0x01, OP_END], vec![]),
        ];
        for (data, references) in programs {
            let mut e = vm(data, references);
            e.step();
            e.force_cancel();
            assert_eq!(
                e.step(),
                StepResult::Cancelled,
                "the force-cancelled VM ends cancelled"
            );
            assert_eq!(e.park(), Park::None, "a cancelled VM is not parked");
        }
        // The Hold variant needs its hold armed before the step.
        let mut e = vm(
            vec![
                OP_WAITSCHEDULOR,
                0xF8,
                0xFF,
                0xFF,
                0x7F,
                0,
                0,
                0,
                0,
                b'm',
                b'a',
                b'i',
                b'n',
                OP_END,
            ],
            vec![],
        );
        e.hold_action_pending(ActorLookup::EVENT_ENTITY, *b"main");
        e.step();
        e.force_cancel();
        assert_eq!(e.step(), StepResult::Cancelled);
    }

    #[test]
    fn entity_valid_branches_on_the_pool() {
        const NPC: u32 = 0x0100_02C5;
        let program = |references: Vec<u32>| {
            let mut data = vec![OP_ENTITY_VALID];
            data.extend_from_slice(&REF0);
            data.extend_from_slice(&8u16.to_le_bytes());
            data.extend_from_slice(&[OP_WAIT, 0x01, 0x80]);
            data.push(OP_END);
            (data, references)
        };
        let (data, references) = program(vec![NPC]);
        let mut e = vm(data, references);
        e.set_actor_types(&bridge_types(NPC));
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "a published entity runs on past the opcode into the wait"
        );
        let (data, references) = program(vec![0x0100_9999]);
        let mut e = vm(data, references);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "an unpublished entity takes the else-target"
        );
        assert_eq!(e.exec_pointer(), 8);
        let (data, references) = program(vec![ActorLookup::EVENT_ENTITY.0]);
        let mut e = vm(data, references);
        e.set_actor_types(&bridge_types(NPC));
        assert_eq!(
            e.step(),
            StepResult::Done,
            "a reserved selector names no pool entry"
        );
    }

    /// 0xC1 emits the stop-all for a resolved actor and parks one frame; an
    /// actor retail's GetActorIndex would drop emits nothing but still parks
    /// (research/XiEvents/OpCodes/0x00C1.md).
    #[test]
    fn kill_last_action_stops_the_resolved_actor_and_yields() {
        const NPC: u32 = 0x0100_02C5;
        let program = |actor: u32| {
            let mut data = vec![OP_KILL_LAST_ACTION];
            data.extend_from_slice(&actor.to_le_bytes());
            data.push(OP_END);
            data
        };
        let mut e = vm(program(NPC), vec![]);
        e.set_actor_types(&bridge_types(NPC));
        assert_eq!(e.step(), StepResult::Waiting, "the kill yields its frame");
        assert_eq!(
            e.take_cues(),
            vec![EventCue::ActorStopAction {
                actor: ActorLookup(NPC),
                key: None,
            }]
        );
        e.tick(1.0 / 60.0);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 5);
        let mut e = vm(program(0x0100_9999), vec![]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "an unresolved actor still yields"
        );
        assert!(
            e.take_cues().is_empty(),
            "an actor retail would drop emits nothing"
        );
        e.tick(1.0 / 60.0);
        assert_eq!(e.step(), StepResult::Done);
    }

    /// `OP_MAPSCHEDULOR` starts the zone-level routine (kuluu resolves the key
    /// out of the current zone's own model DAT); the key sits at @9 like the
    /// WAIT family's, and both actors ride along for the host
    /// (research/XiEvents/OpCodes/0x002D.md).
    #[test]
    fn mapschedulor_emits_the_zone_routine_cue() {
        /// A literal server id with no References entry: it resolves to itself.
        const ACTOR1: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut data = vec![OP_MAPSCHEDULOR];
        data.extend_from_slice(&ACTOR1.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key);
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 13);
        assert_eq!(
            e.take_cues(),
            vec![EventCue::ZoneScheduler {
                key,
                actor1: ActorLookup(ACTOR1),
                actor2: ActorLookup(0),
            }]
        );
    }

    #[test]
    fn waitmapschedulor_parks_on_the_zone_hold() {
        let key: [u8; 4] = *b"abcd";
        let program = || {
            let mut data = vec![OP_WAITMAPSCHEDULOR];
            data.extend_from_slice(&0u32.to_le_bytes());
            data.extend_from_slice(&0u32.to_le_bytes());
            data.extend_from_slice(&key);
            data.push(OP_END);
            vm(data, vec![])
        };
        let mut e = program();
        assert_eq!(
            e.step(),
            StepResult::Done,
            "no zone hold: the wait falls through"
        );
        let mut e = program();
        e.hold_action(ActorLookup::ZONE, key, WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the zone hold parks the wait"
        );
        e.tick(1.1);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "an expired hold falls through to END"
        );
    }

    /// The same-batch bridge parks the wait until the host arms the hold from
    /// the drained cue; the armed hold then takes over from the bridge.
    #[test]
    fn mapschedulor_and_its_wait_in_one_batch_bridge_until_the_cues_drain() {
        let key: [u8; 4] = *b"abcd";
        let mut data = vec![OP_MAPSCHEDULOR];
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key);
        data.push(OP_WAITMAPSCHEDULOR);
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key);
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the wait parks on the zone routine its own loader just started"
        );
        assert_eq!(e.take_cues().len(), 1);
        e.hold_action(ActorLookup::ZONE, key, WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the armed hold keeps the wait parked"
        );
    }

    #[test]
    fn waitschedulor_holds_while_action_runs_then_falls_through() {
        /// A literal server id with no References entry: it resolves to itself.
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut data = vec![OP_WAITSCHEDULOR];
        data.extend_from_slice(&ACTOR.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key);
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "an armed hold parks the wait"
        );
        e.tick(0.5);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "half a second in still holds"
        );
        e.tick(0.6);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "an expired hold falls through to END"
        );
    }

    /// No hold armed: the wait advances immediately instead of parking.
    #[test]
    fn waitschedulor_with_no_hold_falls_through() {
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut data = vec![OP_WAITSCHEDULOR];
        data.extend_from_slice(&ACTOR.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key);
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
    }

    fn waitschedulor_program(actor: u32, key: [u8; 4]) -> Vec<u8> {
        let mut data = vec![OP_WAITSCHEDULOR];
        data.extend_from_slice(&actor.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key);
        data.push(OP_END);
        data
    }

    /// The pending hold has no length: no amount of host clock releases it;
    /// only the renderer's finish report does.
    #[test]
    fn waitschedulor_parks_on_a_pending_hold_until_the_renderer_reports() {
        /// A literal server id with no References entry: it resolves to itself.
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"kue0";
        let mut e = vm(waitschedulor_program(ACTOR, key), vec![]);
        e.hold_action_pending(ActorLookup(ACTOR), key);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "a pending 0x2C hold parks the wait"
        );
        e.tick(3600.0);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the pending hold does not decay on the VM's clock"
        );
        e.release_action_hold(ActorLookup(ACTOR), key);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the renderer's finish report releases the hold"
        );
    }

    /// A stray report (the routine was stopped or the event ended) must not
    /// panic or touch a timed hold for the same pair.
    #[test]
    fn release_action_hold_is_a_noop_when_nothing_is_pending() {
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut e = vm(waitschedulor_program(ACTOR, key), vec![]);
        e.release_action_hold(ActorLookup(ACTOR), key);
        e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC);
        e.release_action_hold(ActorLookup(ACTOR), key);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the timed hold survives a stray release"
        );
    }

    #[test]
    fn a_timed_hold_supersedes_a_pending_hold_for_the_same_pair() {
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut e = vm(waitschedulor_program(ACTOR, key), vec![]);
        e.hold_action_pending(ActorLookup(ACTOR), key);
        e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC);
        e.tick(1.1);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "last-arm-wins: the timed hold expired and nothing pending remains"
        );
    }

    #[test]
    fn a_pending_hold_supersedes_a_timed_hold_for_the_same_pair() {
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut e = vm(waitschedulor_program(ACTOR, key), vec![]);
        e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC);
        e.hold_action_pending(ActorLookup(ACTOR), key);
        e.tick(1.1);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "last-arm-wins: only the pending hold remains"
        );
        e.release_action_hold(ActorLookup(ACTOR), key);
        assert_eq!(e.step(), StepResult::Done);
    }

    fn loadextscheduler_then_wait_program(actor: u32, key: [u8; 4]) -> Vec<u8> {
        let mut data = vec![OP_LOADEXTSCHEDULER];
        data.extend_from_slice(&REF0);
        data.extend_from_slice(&actor.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key);
        data.push(OP_WAITSCHEDULOR);
        data.extend_from_slice(&actor.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key);
        data.push(OP_END);
        data
    }

    /// The `OP_LOADEXTSCHEDULER` gate's accepted Type for the bridge tests'
    /// literal actor, installed under the target index retail's GetActorIndex
    /// resolves it to.
    fn bridge_types(actor: u32) -> std::collections::HashMap<u32, u8> {
        let mut m = std::collections::HashMap::new();
        m.insert(actor & LOOKUP_TARGET_INDEX_MASK, 2u8);
        m
    }

    #[test]
    fn loadextscheduler_and_its_wait_in_one_batch_bridge_until_the_cues_drain() {
        /// A literal server id with no References entry: it resolves to itself.
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut e = vm(loadextscheduler_then_wait_program(ACTOR, key), vec![5]);
        e.set_actor_types(&bridge_types(ACTOR));
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the wait parks on the action its own loader just started"
        );
        assert_eq!(e.take_cues().len(), 1);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "draining without a confirming hold spends the bridge; the wait falls through"
        );
    }

    /// The host arms the hold from the drained cue; it takes over from the
    /// bridge.
    #[test]
    fn loadextscheduler_bridge_hands_off_to_the_host_armed_hold() {
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut e = vm(loadextscheduler_then_wait_program(ACTOR, key), vec![5]);
        e.set_actor_types(&bridge_types(ACTOR));
        assert_eq!(e.step(), StepResult::Waiting);
        assert_eq!(e.take_cues().len(), 1);
        e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the armed hold keeps the wait parked"
        );
        e.tick(1.1);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "an expired hold falls through to END"
        );
    }

    /// `OP_WAITLOADSCHEDULER` layout: file @1, actor1 @3, actor2 @7, key @11.
    #[test]
    fn waitloadscheduler_reads_actor_at_3_and_key_at_11() {
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut data = vec![OP_WAITLOADSCHEDULER];
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&ACTOR.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&key);
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "0x55 must read actor @3 and key @11"
        );
    }

    /// A `OP_MESSAGE` whose ref index selects References[0] (900), then
    /// `OP_MESWAIT`, END.
    #[test]
    fn message_then_meswait_yields_then_resumes() {
        let mut e = vm(vec![OP_MESSAGE, 0x00, 0x80, OP_MESWAIT, OP_END], vec![900]);
        assert_eq!(
            e.step(),
            StepResult::AwaitMessage(EventMessage {
                message_id: 900,
                speaker_index: Some(5),
                params: vec![],
            })
        );
        // Still parked on MESWAIT until dismissed: retail yields every tick
        // with the open flag up, so a re-step reports Waiting rather than an
        // ack that would dismiss the displayed frame
        // (research/XiEvents/OpCodes/0x0023.md).
        assert_eq!(e.step(), StepResult::Waiting);
        e.dismiss_message();
        assert_eq!(e.step(), StepResult::Done);
    }

    /// The trigger packet's num[8] rides along on both yield kinds.
    #[test]
    fn params_flow_through_message_and_choice() {
        let params = vec![7, -1, 42];
        let data = vec![
            OP_MESSAGE,
            0x00,
            0x80,
            OP_MESWAIT,
            OP_QUERY,
            0x00,
            0x80,
            0x01,
            0x80,
            0x00,
            0x00,
            OP_QUERYWAIT,
            OP_END,
        ];
        let mut e = EventVm::start(&block(data, vec![900, 0]), 7, 5, params.clone()).unwrap();
        assert_eq!(
            e.step(),
            StepResult::AwaitMessage(EventMessage {
                message_id: 900,
                speaker_index: Some(5),
                params: params.clone(),
            })
        );
        e.dismiss_message();
        assert_eq!(
            e.step(),
            StepResult::AwaitChoice(EventChoice {
                message_id: 900,
                speaker_index: 5,
                default_index: 0,
                params,
            })
        );
    }

    #[test]
    fn message_id_from_work_local_zero_until_set() {
        // ref index 5 (a WorkLocal slot, unset) -> message_id 0.
        let mut e = vm(vec![OP_MESSAGE, 5, 0, OP_MESWAIT, OP_END], vec![]);
        assert_eq!(
            e.step(),
            StepResult::AwaitMessage(EventMessage {
                message_id: 0,
                speaker_index: Some(5),
                params: vec![],
            })
        );
    }

    /// Case 1 jumps to target when references[0] equals references[0]. Layout:
    /// [0] = `OP_IF`, [1..3] = v1 (ref index 0), [3..5] = v2 (ref index 0),
    /// [5] = kind 1, [6..8] = val3 = 9 (absolute into EventData), [8] = a
    /// filler byte the jump skips, [9] = END.
    #[test]
    fn if_equal_case1_branches_to_target() {
        let data = vec![OP_IF, 0, 128, 0, 128, 1, 9, 0, POISON, OP_END];
        let mut e = vm(data, vec![42]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 9);
    }

    /// The taken-branch target is absolute into EventData, not relative to the
    /// IF: with the IF at 5 and val3 = 20, retail lands on 20; a relative read
    /// would land on 25, past this program's END. Layout: [0..5] WZ[0] =
    /// ref[0] (=5); [5..13] IF case 1 (jump when equal), v1 = WZ[0], v2 =
    /// ref[1] (=5), val3 = 20; [13..20) filler; [20..23] WZ[2] = 1; [23] END.
    #[test]
    fn if_taken_branch_target_is_absolute_into_event_data() {
        let mut data = seed().to_vec();
        data.extend_from_slice(&[OP_IF, DST[0], DST[1], SRC[0], SRC[1], 1, 20, 0]);
        data.extend_from_slice(&[POISON; 7]);
        data.extend_from_slice(&[OP_SET_ONE, 0x02, 0x10]);
        data.push(OP_END);
        let mut e = vm(data, vec![5, 5]);

        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 23, "must land on END past the SET_ONE");
        assert_eq!(
            e.work_zone(2),
            1,
            "the absolute target's instruction must run"
        );
    }

    #[test]
    fn if_equal_case1_falls_through_when_unequal() {
        // references[0]=1 vs references[1]=2 -> not equal -> fall through (+8) to END at 8.
        let data = vec![OP_IF, 0x00, 0x80, 0x01, 0x80, 0x01, 0xFF, 0x00, OP_END];
        let mut e = vm(data, vec![1, 2]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 8);
    }

    #[test]
    fn jump_and_return() {
        // 0x1A jump to subroutine at 6, which is 0x1B return -> back to offset 3 -> END.
        let data = vec![OP_JUMP, 0x06, 0x00, OP_END, 0xFF, 0xFF, OP_RETURN];
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 3);
    }

    /// 0x82 RANGE_RECT hit-tests the event entity's tracked position against the
    /// zone range rect named by the u32 at +1: a hit advances 7, a miss (outside
    /// the box, or no rect) jumps to the u16 at +5
    /// (research/XiEvents/OpCodes/0x0082.md).
    #[test]
    fn range_rect_hit_tests_the_event_entity() {
        use ffxi_dat::datid::DatId;
        use ffxi_dat::zone_interaction::ZoneInteraction;
        fn box_rect(source: [u8; 4]) -> ZoneInteraction {
            ZoneInteraction {
                position: [0.0, 0.0, 0.0],
                rect_class: 0,
                orientation: [0.0, 0.0, 0.0],
                size: [10.0, 10.0, 10.0],
                source_id: DatId(source),
                dest_id: None,
                param: 0,
                terrain_flags: 0,
                map_id: 0,
                elevator_bottom_y: 0.0,
                elevator_top_y: 0.0,
            }
        }
        let source: [u8; 4] = *b"test";
        let rect_id = u32::from_le_bytes(source);
        let rects = std::sync::Arc::new(vec![box_rect(source)]);

        let mut data = vec![OP_RANGE_RECT];
        data.extend_from_slice(&rect_id.to_le_bytes());
        data.extend_from_slice(&10u16.to_le_bytes());
        data.push(OP_END);
        data.push(0xFF);
        data.push(0xFF);
        data.push(OP_END);
        let dat = std::sync::Arc::new(ffxi_dat::event_dat::EventDat {
            blocks: vec![block(vec![OP_END], vec![])],
        });

        let mut e = vm(data.clone(), vec![]);
        e.set_zone_rects(rects.clone());
        e.attach_scene(
            dat.clone(),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition {
                x: 0,
                y: 0,
                z: 0,
                heading: 0,
            },
        );
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 7, "a hit advances 7");

        let mut e = vm(data.clone(), vec![]);
        e.set_zone_rects(rects.clone());
        e.attach_scene(
            dat.clone(),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition {
                x: 6000,
                y: 0,
                z: 0,
                heading: 0,
            },
        );
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 10, "a miss jumps to the u16 at +5");

        let mut e = vm(data.clone(), vec![]);
        e.attach_scene(
            dat.clone(),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition {
                x: 0,
                y: 0,
                z: 0,
                heading: 0,
            },
        );
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 10, "no rect table misses");
    }

    /// The actor-driven waits all reduce to retail's own "no such entity"
    /// advance here, and the width is load-bearing: each carries an actor
    /// lookup the VM steps over blind.
    #[test]
    fn actor_early_exit_opcodes_skip_by_size_and_continue() {
        for (op, size) in [
            (OP_LOADWAIT, LOADWAIT_SIZE),
            (OP_TURNCHECK, LOADWAIT_SIZE),
            (OP_MAPLOAD, MAPLOAD_SIZE),
            (OP_MAPLOAD_KEEP, MAPLOAD_SIZE),
            (OP_MUSICREADWAIT, YIELD_SIZE),
            (OP_YIELD, YIELD_SIZE),
        ] {
            assert_eq!(
                OPCODE_META[op as usize].size as usize, size,
                "op 0x{op:02X} size drifted from research/XiEvents/OpCodes"
            );
            let mut data = vec![op];
            data.extend(std::iter::repeat_n(0u8, size - 1));
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            assert_eq!(
                e.step(),
                StepResult::Done,
                "op 0x{op:02X} should run to END"
            );
            assert_eq!(e.exec_pointer(), size, "op 0x{op:02X} advanced wrong size");
        }
    }

    /// 0x6C emits the fade cue and parks the script for the authored frame
    /// count; a zero-length fade still costs one frame, retail's `AlphaTime`
    /// 0 → 1 (research/XiEvents/OpCodes/0x006C.md).
    #[test]
    fn transpar_opcode_parks_for_its_fade_length() {
        let program = || {
            let mut data = vec![OP_TRANSPAR];
            data.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
            data.extend_from_slice(&REF0);
            data.extend_from_slice(&REF1);
            data.push(OP_END);
            data
        };
        let mut e = vm(program(), vec![128, 60]);
        assert_eq!(e.step(), StepResult::Waiting, "the fade is running");
        assert_eq!(
            e.take_cues(),
            [EventCue::Transpar {
                actor: ActorLookup(NPC_SERVER_ID),
                end_alpha: 128,
                duration_frames: 60,
            }]
        );
        e.tick(0.5);
        assert_eq!(e.step(), StepResult::Waiting, "half way is still waiting");
        e.tick(0.6);
        assert_eq!(e.step(), StepResult::Done);

        let mut e = vm(program(), vec![0, 0]);
        assert_eq!(e.step(), StepResult::Waiting);
        assert_eq!(
            e.take_cues(),
            [EventCue::Transpar {
                actor: ActorLookup(NPC_SERVER_ID),
                end_alpha: 0,
                duration_frames: 1,
            }],
            "a zero-length fade reads as one frame"
        );
    }

    /// 0x3E BITTEST program: bit index from References[1], work word named by
    /// `word_operand`, branch target `target`.
    fn bit_test_program(word_operand: [u8; 2], target: u8) -> Vec<u8> {
        let mut data = vec![OP_BITTEST];
        data.extend_from_slice(&word_operand);
        data.extend_from_slice(&[0x01, 0x80]); // bit index: References[1]
        data.extend_from_slice(&[target, 0x00]);
        data
    }

    #[test]
    fn op_3e_bit_test_takes_the_set_branch() {
        // WorkLocal[10] = References[0] = 1, then test its bit 0 (References[1]).
        let mut data = vec![OP_GET_STORE, 0x0A, 0x00, 0x00, 0x80];
        data.extend_from_slice(&bit_test_program([0x0A, 0x00], 13));
        data.push(OP_END); // 12: the set branch
        data.push(0xFF); // 13: the clear branch must not run
        let mut e = vm(data, vec![1, 0]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 12);
    }

    #[test]
    fn op_3e_bit_test_jumps_when_clear() {
        // WorkLocal[10] is unset, so bit 0 is clear and the u16 target is taken.
        let mut data = bit_test_program([0x0A, 0x00], 8);
        data.push(0xFF); // 7: the set branch must not run
        data.push(OP_END); // 8: the branch target
        let mut e = vm(data, vec![0, 0]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 8);
    }

    /// Bit 32 lives in the NEXT work slot: the index shift is what selects it,
    /// so dropping `getworkofs`' shift argument would read slot 10 and branch
    /// the other way.
    #[test]
    fn op_3e_bit_test_index_selects_the_next_work_word() {
        // WorkLocal[11] = References[0] = 1; test bit 32 (References[1]) of the
        // slot named as WorkLocal[10].
        let mut data = vec![OP_GET_STORE, 0x0B, 0x00, 0x00, 0x80];
        data.extend_from_slice(&bit_test_program([0x0A, 0x00], 13));
        data.push(OP_END); // 12: the set branch
        data.push(0xFF); // 13: the clear branch must not run
        let mut e = vm(data, vec![1, 32]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 12);
    }

    /// 0x7F yields for a selection exactly like 0x25.
    #[test]
    fn op_7f_querywait2_yields_then_resumes() {
        let data = vec![
            OP_QUERY,
            0x00,
            0x80,
            0x01,
            0x80,
            0x00,
            0x00,
            OP_QUERYWAIT2,
            OP_END,
        ];
        let expected = StepResult::AwaitChoice(EventChoice {
            message_id: 500,
            speaker_index: 5,
            default_index: 0,
            params: vec![],
        });
        let mut e = vm(data, vec![500, 0]);
        assert_eq!(e.step(), expected);
        assert_eq!(e.step(), expected, "still awaiting until a choice is made");
        e.select_choice(Some(1));
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 8);
    }

    /// 8700's favorites branch: a bare 0x25 after the menu that answered the
    /// previous 0x25. Retail steps past it (`!PTR_TalkWinFlag`) and the IF
    /// behind it reads the slot the player already picked
    /// (research/XiEvents/OpCodes/0x0025.md). Layout: [0..7] QUERY,
    /// [7] QUERYWAIT, [8] QUERYWAIT (bare), [9] END.
    #[test]
    fn op_25_querywait_with_no_menu_open_skips_and_keeps_the_last_selection() {
        let data = vec![
            OP_QUERY,
            0x00,
            0x80,
            0x01,
            0x80,
            0x00,
            0x00,
            OP_QUERYWAIT,
            OP_QUERYWAIT,
            OP_END,
        ];
        let mut e = vm(data, vec![500, 0]);
        assert!(matches!(e.step(), StepResult::AwaitChoice(_)));
        e.select_choice(Some(2));
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 9, "the bare QUERYWAIT is one byte");
        assert_eq!(e.work_zone(0), 2, "the earlier selection survives the skip");
    }

    /// A 0x25 with nothing armed at all: the event starts on it and runs on.
    #[test]
    fn op_25_querywait_as_the_first_opcode_runs_on() {
        let mut e = vm(vec![OP_QUERYWAIT, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 1);
    }

    /// A message frame with no MESWAIT behind it, then a bare 0x25: the frame
    /// shows, the host dismisses it, and the 0x25 skips instead of parking.
    #[test]
    fn op_25_querywait_after_a_dismissed_message_skips() {
        let data = vec![OP_MESSAGE, 0x00, 0x80, OP_QUERYWAIT, OP_END];
        let mut e = vm(data, vec![900]);
        assert!(matches!(e.step(), StepResult::AwaitMessage(_)));
        e.dismiss_message();
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 4);
    }

    /// 0x7F takes the same no-window exit as 0x25.
    #[test]
    fn op_7f_querywait2_with_no_menu_open_skips() {
        let mut e = vm(vec![OP_QUERYWAIT2, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 1);
    }

    /// Unlike 0x25, a cancelled menu stores 255 and the event runs on.
    #[test]
    fn op_7f_querywait2_does_not_cancel_on_a_cancelled_choice() {
        let data = vec![
            OP_QUERY,
            0x00,
            0x80,
            0x01,
            0x80,
            0x00,
            0x00,
            OP_QUERYWAIT2,
            OP_END,
        ];
        let mut e = vm(data, vec![500, 0]);
        assert!(matches!(e.step(), StepResult::AwaitChoice(_)));
        e.select_choice(None);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 8);
        assert_eq!(e.work_zone(0), CHOICE_CANCELLED_QUERYWAIT2 as i32);
    }

    /// A sub byte with no documented width must stop the VM, not fall back to
    /// the table's widest case — that would advance 0xB6 by 20 over a 2-byte
    /// instruction and start executing operands as opcodes.
    #[test]
    fn sub_width_opcodes_stop_rather_than_falling_back_to_the_fixed_size() {
        for (op, sub) in [(OP_LOOKSET, 0x16u8), (OP_STRINGOPS, 0x07)] {
            let mut data = vec![op, sub];
            data.extend(std::iter::repeat_n(
                0u8,
                OPCODE_META[op as usize].size as usize,
            ));
            let mut e = vm(data, vec![]);
            assert_eq!(
                e.step(),
                StepResult::Unimplemented(op),
                "op 0x{op:02X} sub 0x{sub:02X} must stop"
            );
            assert_eq!(e.exec_pointer(), 0, "op 0x{op:02X} must not advance");
        }
    }

    /// The sub-byte families advance by their case's width, and reach END.
    #[test]
    fn sub_width_opcodes_advance_by_their_case_width() {
        for (op, sub, width) in [
            (OP_LOOKSET, 0x0Bu8, 20usize),
            (OP_EVENTPOSSET, 0x01, 2),
            (OP_LOADROOM, 0x00, 4),
            (OP_ITEMINFO, 0x02, 14),
            (OP_ENTITYSPEED, 0x05, 7),
            (OP_MOVE, 0x00, 8),
            (OP_WINDOW, 0x14, 12),
            (OP_MENU, 0x20, 16),
            (OP_RENDERFLAG, 0x1B, 6),
            (OP_REQRESET, 0x01, 7),
            (OP_NAMESET, 0x00, 4),
            (OP_SUBSCHED, 0x05, 18),
            (OP_STRINGOPS, 0x08, 23),
            (OP_STATUSSET, 0x04, 8),
        ] {
            let mut data = vec![op, sub];
            data.extend(std::iter::repeat_n(0u8, width - 2));
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            assert_eq!(
                e.step(),
                StepResult::Done,
                "op 0x{op:02X} sub 0x{sub:02X} should run to END"
            );
            assert_eq!(
                e.exec_pointer(),
                width,
                "op 0x{op:02X} sub 0x{sub:02X} advanced wrong size"
            );
        }
    }

    /// The player-input polls must stop the VM. Retail leaves `ExecPointer`
    /// where it is until the player has typed a value or picked an entry, then
    /// advances while storing the answer; skipping by the case width instead
    /// runs the script on as if the player had answered, with the destination
    /// work slot holding 0 (research/XiEvents/OpCodes/0x0071.md, 0x00CC.md,
    /// 0x00B4.md).
    #[test]
    fn input_wait_polls_stop_rather_than_advancing_unanswered() {
        for (op, sub) in [
            (OP_MENU, 0x01u8),
            (OP_MENU, 0x02),
            (OP_MENU, 0x11),
            (OP_MENU, 0x13),
            (OP_MENU, 0x31),
            (OP_MENU, 0x41),
            (OP_ITEMINFO, 0x11),
        ] {
            let width = crate::opcode_meta::sub_size(op, sub)
                .unwrap_or_else(|| panic!("op 0x{op:02X} sub 0x{sub:02X} has a documented width"))
                as usize;
            let mut data = vec![op, sub];
            data.extend(std::iter::repeat_n(0u8, width - 2));
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            assert_eq!(
                e.step(),
                StepResult::Unimplemented(op),
                "op 0x{op:02X} sub 0x{sub:02X} must stop for input"
            );
            assert_eq!(
                e.exec_pointer(),
                0,
                "op 0x{op:02X} sub 0x{sub:02X} must not advance"
            );
        }
    }

    /// An unhandled non-jumping opcode of size 1 is skipped by its size and
    /// reaches END (research/XiEvents/OpCodes/0x0030.md).
    #[test]
    fn unknown_nonjump_opcode_skipped_by_size() {
        let mut e = vm(vec![0x30, 0x30, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
    }

    /// Armed at event start; `OP_CANCEL_DISARM` (event 503's second opcode)
    /// locks the event in, and `OP_CANCEL_ARM` re-arms it for interactive
    /// beats.
    #[test]
    fn cancel_flag_disarms_and_rearms() {
        let mut e = vm(vec![OP_CANCEL_DISARM, OP_END], vec![]);
        assert!(e.cancel_armed(), "armed at start");
        assert_eq!(e.step(), StepResult::Done);
        assert!(!e.cancel_armed(), "0x42 disarms ESC-cancel");

        let mut e = vm(vec![OP_CANCEL_DISARM, OP_CANCEL_ARM, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert!(e.cancel_armed(), "0x2E re-arms after 0x42");
    }

    #[test]
    fn query_then_querywait_yields_choice_then_resumes() {
        // QUERY(msg=ref0=500, default=ref1=0) -> QUERYWAIT -> END.
        let data = vec![
            OP_QUERY,
            0x00,
            0x80,
            0x01,
            0x80,
            0x00,
            0x00,
            OP_QUERYWAIT,
            OP_END,
        ];
        let expected = StepResult::AwaitChoice(EventChoice {
            message_id: 500,
            speaker_index: 5,
            default_index: 0,
            params: vec![],
        });
        let mut e = vm(data, vec![500, 0]);
        assert_eq!(e.step(), expected);
        assert_eq!(e.step(), expected, "still awaiting until a choice is made");
        e.select_choice(Some(1));
        assert_eq!(e.step(), StepResult::Done);
    }

    #[test]
    fn cancelled_message_ends_event_at_meswait() {
        let mut e = vm(vec![OP_MESSAGE, 0x00, 0x80, OP_MESWAIT, OP_END], vec![900]);
        assert!(matches!(e.step(), StepResult::AwaitMessage(_)));
        e.cancel_message();
        assert_eq!(e.step(), StepResult::Cancelled);
    }

    #[test]
    fn cancelled_choice_ends_event() {
        let data = vec![
            OP_QUERY,
            0x00,
            0x80,
            0x01,
            0x80,
            0x00,
            0x00,
            OP_QUERYWAIT,
            OP_END,
        ];
        let mut e = vm(data, vec![500, 0]);
        assert!(matches!(e.step(), StepResult::AwaitChoice(_)));
        e.select_choice(None);
        assert_eq!(e.step(), StepResult::Cancelled);
    }

    #[test]
    fn op_03_get_store_copies_value() {
        // 0x03: copy References[0]=55 into WorkLocal[10], then MESSAGE reads it.
        let data = vec![
            OP_GET_STORE,
            0x0A,
            0x00, // dst: WorkLocal[10]
            0x00,
            0x80, // src: References[0]
            OP_MESSAGE,
            0x0A,
            0x00, // msg id from WorkLocal[10]
            OP_MESWAIT,
            OP_END,
        ];
        let mut e = vm(data, vec![55]);
        assert_eq!(
            e.step(),
            StepResult::AwaitMessage(EventMessage {
                message_id: 55,
                speaker_index: Some(5),
                params: vec![],
            })
        );
    }

    #[test]
    fn op_21_execend_finishes() {
        let mut e = vm(vec![OP_EXECEND], vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.step(), StepResult::Done);
    }

    #[test]
    fn sleep_wait_turn_opcodes_advance() {
        // 0x6F (+1), 0x70 (+1), 0x1C (+3 over its 2 operand bytes) then END.
        // The timers each cost a slice of host clock before advancing.
        let data = vec![OP_SLEEP, OP_TURNWAIT, OP_WAIT, 0x00, 0x00, OP_END];
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Waiting, "0x6F arms its fixed 16");
        e.tick(1.0);
        assert_eq!(e.step(), StepResult::Waiting, "0x1C arms (zero-length)");
        e.tick(1.0);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 5);
    }

    #[test]
    fn setbitwork_matches_xievents_example() {
        // research/XiEvents/OpCodes/0x0040.md, "Call 1": v1=0, v2=0x0F, src(5)=0,
        // v4(7)=0x0008 -> Set(5)=0x0008. idx5 is WorkLocal[10] (read 0, written
        // back), then MESSAGE reads it.
        let data = vec![
            OP_SETBITWORK,
            0x01,
            0x80, // v1: References[1]=0
            0x02,
            0x80, // v2: References[2]=15
            0x0A,
            0x00, // src/dst: WorkLocal[10]
            0x03,
            0x80, // v4: References[3]=8
            OP_MESSAGE,
            0x0A,
            0x00,
            OP_MESWAIT,
            OP_END,
        ];
        let mut e = vm(data, vec![0, 0, 15, 8]);
        assert_eq!(
            e.step(),
            StepResult::AwaitMessage(EventMessage {
                message_id: 8,
                speaker_index: Some(5),
                params: vec![],
            })
        );
    }

    #[test]
    fn choice_result_drives_if_branch() {
        // QUERY -> QUERYWAIT -> IF(work_zone[0] == ref2) jump to END at 19.
        // ref0=500 msg, ref1=0 default, ref2=1 compare value.
        let data = vec![
            OP_QUERY,
            0x00,
            0x80,
            0x01,
            0x80,
            0x00,
            0x00,         // 0..6: QUERY
            OP_QUERYWAIT, // 7
            OP_IF,
            0x00,
            0x10,
            0x02,
            0x80,
            0x07,
            0x13,
            0x00, // 8..15: if work_zone[0]==ref2 -> END at 19 (absolute target)
            0xFF,
            0xFF,
            0xFF,   // 16..18: fall-through poison (must not run)
            OP_END, // 19
        ];
        let mut e = vm(data, vec![500, 0, 1]);
        assert!(matches!(e.step(), StepResult::AwaitChoice(_)));
        e.select_choice(Some(1)); // work_zone[0] = 1, matching ref2
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 19);
    }

    /// Server id of a synthetic NPC; `& 0x3FF` is its target index.
    const NPC_SERVER_ID: u32 = 0x0100_02C5;
    const NPC_TARGET_INDEX: u16 = 0x2C5;
    /// `XiEvent::GetActorIndex` lookup selecting the event entity
    /// (research/XiEvents/Event VM Functions.md).
    const LOOKUP_EVENT_ENTITY: u32 = 0x7FFF_FFF8;
    /// References[0] in the message tests below.
    const MSG_ID: u32 = 900;
    /// Operand selecting References[0] (the [`REFERENCE_FLAG`] marker).
    const REF0: [u8; 2] = [0x00, 0x80];
    /// Operand selecting References[1].
    const REF1: [u8; 2] = [0x01, 0x80];
    /// Operand selecting References[2].
    const REF2: [u8; 2] = [0x02, 0x80];
    /// Operand selecting References[3].
    const REF3: [u8; 2] = [0x03, 0x80];
    /// Operand selecting References[7].
    const REF7: [u8; 2] = [7, (REFERENCE_FLAG >> 8) as u8];

    /// Bytecode for one message opcode followed by MESWAIT + END.
    fn message_program(op: u8, operands: &[u8]) -> Vec<u8> {
        let mut data = vec![op];
        data.extend_from_slice(operands);
        assert_eq!(
            data.len(),
            OPCODE_META[op as usize].size as usize,
            "op 0x{op:02X} operands must fill its documented size"
        );
        data.extend_from_slice(&[OP_MESWAIT, OP_END]);
        data
    }

    fn await_message(op: u8, operands: &[u8], speaker_index: Option<u16>) {
        let mut e = vm(message_program(op, operands), vec![MSG_ID]);
        assert_eq!(
            e.step(),
            StepResult::AwaitMessage(EventMessage {
                message_id: MSG_ID,
                speaker_index,
                params: vec![],
            }),
            "op 0x{op:02X} must emit a dialog frame"
        );
        assert_eq!(
            e.exec_pointer(),
            OPCODE_META[op as usize].size as usize,
            "op 0x{op:02X} must park on the following MESWAIT"
        );
        e.dismiss_message();
        assert_eq!(e.step(), StepResult::Done);
    }

    /// Sizes the message opcodes advance by, against research/XiEvents/OpCodes.
    #[test]
    fn message_opcode_meta_matches_xievents_docs() {
        for (op, size) in [
            (OP_MESSAGE, 3u8),
            (OP_MESSAGE_ACTOR, 7),
            (OP_MESSAGE_UNNAMED, 3),
            (OP_MESSAGE_UNNAMED_ACTOR, 7),
            (OP_MESSAGE_ACTOR_PAIR, 12),
        ] {
            let meta = OPCODE_META[op as usize];
            assert_eq!(meta.size, size, "op 0x{op:02X} size drifted");
            assert!(!meta.sets_ret, "op 0x{op:02X} does not set RetFlag");
            assert!(!meta.jumps, "op 0x{op:02X} advances linearly");
        }
    }

    /// 0x2B carries its own speaker: a raw server id resolves to its low bits.
    #[test]
    fn actor_message_attributes_resolved_speaker() {
        let mut operands = NPC_SERVER_ID.to_le_bytes().to_vec();
        operands.extend_from_slice(&REF0);
        await_message(OP_MESSAGE_ACTOR, &operands, Some(NPC_TARGET_INDEX));
    }

    /// The event-entity lookup falls back to the VM's own speaker.
    #[test]
    fn actor_message_event_entity_lookup_uses_event_speaker() {
        let mut operands = LOOKUP_EVENT_ENTITY.to_le_bytes().to_vec();
        operands.extend_from_slice(&REF0);
        await_message(OP_MESSAGE_ACTOR, &operands, Some(5));
    }

    /// 0x48 prints with no speaker at all.
    #[test]
    fn unnamed_message_has_no_speaker() {
        await_message(OP_MESSAGE_UNNAMED, &REF0, None);
    }

    /// 0x49 resolves an actor but still prints unnamed.
    #[test]
    fn unnamed_actor_message_has_no_speaker() {
        let mut operands = NPC_SERVER_ID.to_le_bytes().to_vec();
        operands.extend_from_slice(&REF0);
        await_message(OP_MESSAGE_UNNAMED_ACTOR, &operands, None);
    }

    /// 0xB0's first entity is the speaker; the second is the listener.
    #[test]
    fn actor_pair_message_attributes_first_entity() {
        let mut operands = vec![0];
        operands.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        operands.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        operands.extend_from_slice(&REF0);
        await_message(OP_MESSAGE_ACTOR_PAIR, &operands, Some(NPC_TARGET_INDEX));
    }

    /// A set stall flag makes retail return without advancing `ExecPointer`;
    /// stop instead of spinning (research/XiEvents/OpCodes/0x00B0.md).
    #[test]
    fn actor_pair_message_with_stall_flag_stops() {
        let mut operands = vec![1];
        operands.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        operands.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        operands.extend_from_slice(&REF0);
        let mut e = vm(message_program(OP_MESSAGE_ACTOR_PAIR, &operands), vec![]);
        assert_eq!(e.step(), StepResult::Unimplemented(OP_MESSAGE_ACTOR_PAIR));
        assert_eq!(e.exec_pointer(), 0);
    }

    use crate::cue::{
        ExtSchedulerMotion, FourCc, TpcMotionPackages, EVENT_MOTION_BAND_4,
        LOOKUP_TARGET_INDEX_MASK, SCHEDULER_DURATION_FROM_DAT, SCHEDULER_FADE_DAT_ID,
        SCHEDULER_TAG_FADE_IN, SCHEDULER_TAG_FADE_OUT, TPC_PACKAGE_OUT_OF_RANGE,
    };

    /// Run one choreography opcode (padded to its documented width) to END and
    /// return the cues it emitted.
    fn cues_of(op: u8, operands: &[u8], references: Vec<u32>) -> Vec<EventCue> {
        cues_of_with_types(op, operands, references, &std::collections::HashMap::new())
    }

    /// [`cues_of`] with the entity Type table the `OP_LOADEXTSCHEDULER`/
    /// `OP_LOADEXTSCHEDULER2` gate reads installed before the step: a bare VM
    /// has no Type data, and an absent entry is Type 0 — retail's no-back-ptr
    /// value, which refuses `OP_LOADEXTSCHEDULER`.
    fn cues_of_with_types(
        op: u8,
        operands: &[u8],
        references: Vec<u32>,
        types: &std::collections::HashMap<u32, u8>,
    ) -> Vec<EventCue> {
        let mut data = vec![op];
        data.extend_from_slice(operands);
        let width = crate::opcode_meta::sub_size(op, data.get(1).copied().unwrap_or(0))
            .unwrap_or(OPCODE_META[op as usize].size) as usize;
        assert_eq!(
            data.len(),
            width,
            "op 0x{op:02X} operands must fill its width"
        );
        data.push(OP_END);
        let mut e = vm(data, references);
        e.set_actor_types(types);
        assert_eq!(e.step(), StepResult::Done, "op 0x{op:02X} must run to END");
        assert_eq!(e.exec_pointer(), width, "op 0x{op:02X} advanced wrong size");
        e.take_cues()
    }

    /// True when an `OP_WAITSCHEDULOR` after `loader` (with
    /// `loader_operands`, `loader`'s width, and `FILE_OFS`-style padding)
    /// parks on the loader's same-batch start instead of falling through to
    /// END.
    fn wait_after_loader_parks(
        loader: u8,
        loader_operands: &[u8],
        types: &std::collections::HashMap<u32, u8>,
    ) -> bool {
        let mut data = vec![loader];
        data.extend_from_slice(loader_operands);
        data.push(OP_WAITSCHEDULOR);
        data.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        data.extend_from_slice(&[0u8; 4]);
        data.extend_from_slice(b"abcd");
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        e.set_actor_types(types);
        e.step() == StepResult::Waiting
    }

    /// Operand bytes for the 0x45 fade sites the whole retail corpus authors:
    /// work operand -> References[0], both actors the event entity, the fade
    /// tag raw, duration -> References[1].
    fn fade_operands(tag: FourCc) -> Vec<u8> {
        let mut o = REF0.to_vec();
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&tag);
        o.extend_from_slice(&[0x01, 0x80]);
        o
    }

    /// Guard for the emitter/matcher contract: 0x45's tag operand is four raw
    /// ASCII bytes in file order, and the fade pair the host matches on is
    /// exactly what the VM emits from that bytecode.
    #[test]
    fn fade_scheduler_opcode_emits_the_exported_tags_and_dat_id() {
        /// References[0]: the work operand every authored fade site passes.
        const FADE_WORK_OPERAND: u32 = 200;
        for tag in [SCHEDULER_TAG_FADE_OUT, SCHEDULER_TAG_FADE_IN] {
            assert_eq!(
                cues_of(
                    OP_LOADEVENTSCHEDULER2,
                    &fade_operands(tag),
                    vec![FADE_WORK_OPERAND, SCHEDULER_DURATION_FROM_DAT as u32],
                ),
                [EventCue::Scheduler {
                    dat_id: SCHEDULER_FADE_DAT_ID,
                    actor1: ActorLookup::EVENT_ENTITY,
                    actor2: ActorLookup::EVENT_ENTITY,
                    tag,
                    duration: SCHEDULER_DURATION_FROM_DAT,
                }]
            );
        }
    }

    /// 0x73's work operand is a spell animation index; the cue is the spell
    /// effect DAT's `main` on actor1 with actor2 as the target, the gate guard's
    /// Signet (497) and home point (504) casts being the authored cases
    /// (research/XiEvents/OpCodes/0x0073.md).
    #[test]
    fn magic_schedulor_opcode_emits_the_spell_dat_main_routine() {
        const SIGNET_ANIMATION: u32 = 497;
        const HOME_POINT_ANIMATION: u32 = 504;
        for animation in [SIGNET_ANIMATION, HOME_POINT_ANIMATION] {
            let mut operands = REF0.to_vec();
            operands.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
            operands.extend_from_slice(&ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
            assert_eq!(
                cues_of(OP_MAGICSCHEDULOR, &operands, vec![animation]),
                [EventCue::Scheduler {
                    dat_id: MAGIC_DAT_ID_BASE + animation,
                    actor1: ActorLookup::EVENT_ENTITY,
                    actor2: ActorLookup::LOCAL_PLAYER,
                    tag: MAGIC_ROUTINE_TAG,
                    duration: SCHEDULER_DURATION_FROM_DAT,
                }]
            );
        }
    }

    /// A work operand that is not a u16 names no spell DAT: the opcode is
    /// stepped over with no cue, and the program continues.
    #[test]
    fn magic_schedulor_opcode_with_no_animation_emits_nothing() {
        let mut operands = REF0.to_vec();
        operands.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        operands.extend_from_slice(&ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
        assert!(cues_of(OP_MAGICSCHEDULOR, &operands, vec![u32::MAX]).is_empty());
    }

    /// 0x50 ENDSCHEDULOR: kill the tag-named action on actor1, actor2 riding
    /// along; a zero word or four spaces names no routine slot
    /// (research/XiEvents/OpCodes/0x0050.md).
    #[test]
    fn end_schedulor_emits_the_stop_action_cue() {
        const NPC: u32 = 0x0100_02C5;
        let operands = |tag: FourCc| {
            let mut o = NPC.to_le_bytes().to_vec();
            o.extend_from_slice(&NPC.to_le_bytes());
            o.extend_from_slice(&tag);
            o
        };
        assert_eq!(
            cues_of(OP_ENDSCHEDULOR, &operands(*b"sswh"), vec![]),
            [EventCue::ActorStopAction {
                actor: ActorLookup(NPC),
                key: Some(*b"sswh"),
            }]
        );
        for tag in [STOP_TAG_ZERO, STOP_TAG_SPACES] {
            assert_eq!(
                cues_of(OP_ENDSCHEDULOR, &operands(tag), vec![]),
                [EventCue::ActorStopAction {
                    actor: ActorLookup(NPC),
                    key: None,
                }]
            );
        }
    }

    /// 0x52 ENDLOADSCHEDULER_Main: the same kill with the DAT-selector work
    /// operand at +1, which the stop cue does not carry (the arm does not read
    /// it), actor1 at +3, the tag at +11
    /// (research/XiEvents/OpCodes/0x0052.md).
    #[test]
    fn end_loads_scheduler_main_emits_the_stop_action_cue() {
        const NPC: u32 = 0x0100_02C5;
        let mut operands = REF0.to_vec();
        operands.extend_from_slice(&NPC.to_le_bytes());
        operands.extend_from_slice(&NPC.to_le_bytes());
        operands.extend_from_slice(b"sswh");
        assert_eq!(
            cues_of(OP_ENDLOADSCHEDULER_MAIN, &operands, vec![0]),
            [EventCue::ActorStopAction {
                actor: ActorLookup(NPC),
                key: Some(*b"sswh"),
            }]
        );
    }

    /// The 0x52 twins share the layout and the cue, each on its own DAT base
    /// (research/XiEvents/OpCodes/0x00A3.md).
    #[test]
    fn end_loads_scheduler_twin_emits_the_stop_action_cue() {
        const NPC: u32 = 0x0100_02C5;
        let mut operands = REF0.to_vec();
        operands.extend_from_slice(&NPC.to_le_bytes());
        operands.extend_from_slice(&NPC.to_le_bytes());
        operands.extend_from_slice(b"sswh");
        assert_eq!(
            cues_of(OP_ENDLOADSCHED_TWIN_A3, &operands, vec![0]),
            [EventCue::ActorStopAction {
                actor: ActorLookup(NPC),
                key: Some(*b"sswh"),
            }]
        );
    }

    /// The 0x45 twins load their scheduler DAT from their own base plus the raw
    /// work value — the `dat_id_helper` remap is the 0x45-only branch, so a
    /// mid-band reference (400) must land at `base + 400`, not `base + 25937 + 400`
    /// (research/XiEvents/OpCodes/0x0045.md).
    #[test]
    fn scheduler_twin_uses_its_base_without_the_dat_id_helper() {
        const WORK: u32 = 400;
        for (op, base) in [
            (OP_SCHED_TWIN_62, 5012u32),
            (OP_SCHED_TWIN_9F, 51183),
            (OP_SCHED_TWIN_BB, 56685),
            (OP_SCHED_TWIN_C5, 67355),
            (OP_SCHED_TWIN_CD, 70435),
            (OP_SCHED_TWIN_D0, 70691),
            (OP_SCHED_TWIN_D5, 102449),
        ] {
            let mut operands = REF0.to_vec();
            operands.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
            operands.extend_from_slice(&ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
            operands.extend_from_slice(&MAGIC_ROUTINE_TAG);
            operands.extend_from_slice(&REF1);
            assert_eq!(
                cues_of(
                    op,
                    &operands,
                    vec![WORK, SCHEDULER_DURATION_FROM_DAT as u32]
                ),
                [EventCue::Scheduler {
                    dat_id: base + WORK,
                    actor1: ActorLookup::EVENT_ENTITY,
                    actor2: ActorLookup::LOCAL_PLAYER,
                    tag: MAGIC_ROUTINE_TAG,
                    duration: SCHEDULER_DURATION_FROM_DAT,
                }],
                "op 0x{op:02X}"
            );
        }
    }

    /// 0xC4 is the 0x73 cast with a case byte: the key shifts to +2, actor2 to
    /// +8, and the advance is 12 (11 + param1), not the 11 the table records.
    /// The key's reference high byte doubles as actor1's low byte, so actor1 is
    /// a server id whose low byte is 0x80
    /// (research/XiEvents/OpCodes/0x00C4.md, research/XiEvents/OpCodes/0x0073.md).
    #[test]
    fn magic_twin_0xc4_casts_and_advances_twelve() {
        const ANIMATION: u32 = 497;
        const ACTOR1: u32 = 0x0100_0080;
        let mut data = vec![OP_MAGIC_TWIN, 0x00];
        data.extend_from_slice(&REF0);
        data.extend_from_slice(&[0x00, 0x00, 0x01]);
        data.push(0x00);
        data.extend_from_slice(&ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
        data.push(OP_END);
        let mut e = vm(data, vec![ANIMATION]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), MAGIC_TWIN_SIZE, "0xC4 must advance 12");
        assert_eq!(
            e.take_cues(),
            [EventCue::Scheduler {
                dat_id: MAGIC_DAT_ID_BASE + ANIMATION,
                actor1: ActorLookup(ACTOR1),
                actor2: ActorLookup::LOCAL_PLAYER,
                tag: MAGIC_ROUTINE_TAG,
                duration: SCHEDULER_DURATION_FROM_DAT,
            }]
        );
    }

    /// Without scene data, 0x32 arms the speed and 0x1F case 0 walks the event
    /// entity to its goal; the goal reads x@2, z@4, y@6
    /// (research/XiEvents/OpCodes/0x001F.md, research/XiEvents/OpCodes/0x0032.md).
    #[test]
    fn main_speed_arms_the_non_scene_move() {
        const MOVE_CASE_WALK: u8 = 0x00;
        let mut data = vec![OP_MAIN_SPEED];
        data.extend_from_slice(&REF0);
        data.push(crate::opcode_meta::OP_MOVE);
        data.push(MOVE_CASE_WALK);
        data.extend_from_slice(&REF1);
        data.extend_from_slice(&REF2);
        data.extend_from_slice(&REF3);
        data.push(OP_END);
        let mut e = vm(data, vec![10, 20, 40, (-5_i32) as u32]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(
            e.take_cues(),
            [EventCue::ActorMove {
                actor: ActorLookup::EVENT_ENTITY,
                goal: crate::vm::scene::EventPosition {
                    x: 20,
                    z: 40,
                    y: -5,
                    heading: 0,
                },
                speed: 10,
                max_time: None,
            }]
        );
    }

    /// 0x1F case 1 parks on the host-armed move hold and advances once it is
    /// released (research/XiEvents/OpCodes/0x001F.md).
    #[test]
    fn non_scene_move_case1_holds_on_the_move_hold() {
        let program = || vec![crate::opcode_meta::OP_MOVE, 0x01, OP_END];
        let mut e = vm(program(), vec![]);
        e.hold_move(ActorLookup::EVENT_ENTITY, 5.0);
        assert_eq!(e.step(), StepResult::Waiting, "the move is running");
        e.tick(5.0 / WAIT_UNITS_PER_SEC);
        assert_eq!(e.step(), StepResult::Done, "the move is over");
    }

    /// 0x31 mode 0 arms the goal (refs 1/2/3) and the MoveTime budget (ref 0),
    /// advancing ten bytes with no cue (research/XiEvents/OpCodes/0x0031.md).
    #[test]
    fn smove_mode0_arms_the_goal_and_time() {
        let mut data = vec![OP_SMOVE, 0x00];
        data.extend_from_slice(&REF1);
        data.extend_from_slice(&REF2);
        data.extend_from_slice(&REF3);
        data.extend_from_slice(&REF0);
        data.push(OP_END);
        let mut e = vm(data, vec![4000, 20, 40, (-5_i32) as u32]);
        assert_eq!(e.step(), StepResult::Done);
        assert!(e.take_cues().is_empty(), "mode 0 emits no cue");
    }

    /// 0x31 mode 1 walks the event entity to the mode-0 goal at the 0x32
    /// speed, carries the MoveTime budget as the cue's cap, and parks until the
    /// move hold releases it (research/XiEvents/OpCodes/0x0031.md).
    #[test]
    fn smove_mode1_walks_and_parks_on_the_move_hold() {
        let mut data = vec![OP_SMOVE, 0x00];
        data.extend_from_slice(&REF1);
        data.extend_from_slice(&REF2);
        data.extend_from_slice(&REF3);
        data.extend_from_slice(&REF0);
        data.push(OP_SMOVE);
        data.push(0x01);
        data.push(OP_END);
        let mut e = vm(data, vec![4000, 20, 40, (-5_i32) as u32]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "mode 0 arms, mode 1 emits the cue and parks"
        );
        assert_eq!(
            e.take_cues(),
            [EventCue::ActorMove {
                actor: ActorLookup::EVENT_ENTITY,
                goal: crate::vm::scene::EventPosition {
                    x: 20,
                    z: 40,
                    y: -5,
                    heading: 0,
                },
                speed: 0,
                max_time: Some(4.0),
            }]
        );
        e.hold_move(ActorLookup::EVENT_ENTITY, 5.0);
        assert_eq!(e.step(), StepResult::Waiting, "the move is running");
        e.tick(5.0 / WAIT_UNITS_PER_SEC);
        assert_eq!(e.step(), StepResult::Done, "the move is over");
    }

    /// 0x31 mode 1 with no mode-0 goal emits no cue and falls through, the way
    /// retail's zero MovePosition snaps immediately (research/XiEvents/OpCodes/
    /// 0x0031.md).
    #[test]
    fn smove_mode1_without_a_goal_falls_through() {
        let data = vec![OP_SMOVE, 0x01, OP_END];
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert!(e.take_cues().is_empty(), "no goal, no cue");
    }

    /// 0x31's undocumented cases stop the VM rather than guessing a width
    /// (research/XiEvents/OpCodes/0x0031.md).
    #[test]
    fn smove_unknown_case_stops() {
        let data = vec![OP_SMOVE, 0x02, OP_END];
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Unimplemented(OP_SMOVE));
    }

    /// 0x72 mode 0 kicks off the forecast read: it advances past itself and
    /// yields one frame the way retail's async read does, then mode 1 copies
    /// the region's three forecast values into Work_Zone[2..5)
    /// (research/XiEvents/OpCodes/0x0072.md).
    #[test]
    fn getweather_reads_the_forecast_into_work_zone() {
        let mut data = vec![OP_GETWEATHER, 0x00];
        data.extend_from_slice(&REF0);
        data.push(OP_GETWEATHER);
        data.push(0x01);
        data.extend_from_slice(&REF0);
        data.extend_from_slice(&REF1);
        data.push(OP_END);
        let mut head_low = [0u8; ffxi_dat::weather::FORECAST_HEADS_LOW];
        head_low[17] = 5;
        let mut data_low = vec![0u32; 14000];
        data_low[6785] = 111;
        data_low[6786] = 222;
        data_low[6787] = 333;
        let forecast = ffxi_dat::weather::WeatherForecast::from_parts(
            head_low,
            data_low,
            [0u8; ffxi_dat::weather::FORECAST_HEADS_HIGH],
            vec![0u32; 14000],
        );
        let mut e = vm(data, vec![17, 100]);
        e.set_weather_forecast(std::sync::Arc::new(forecast));
        assert_eq!(e.step(), StepResult::Waiting, "mode 0 yields on the read");
        e.tick(1.0 / WAIT_UNITS_PER_SEC);
        assert_eq!(e.step(), StepResult::Done, "mode 1 reads and advances");
        assert_eq!(e.work_zone(2), 111);
        assert_eq!(e.work_zone(3), 222);
        assert_eq!(e.work_zone(4), 333);
        assert!(e.take_cues().is_empty(), "0x72 writes work, not cues");
    }

    /// A VM the host never gave the forecast table advances past 0x72 without
    /// writing, like retail's unresolved early return
    /// (research/XiEvents/OpCodes/0x0072.md).
    #[test]
    fn getweather_without_the_forecast_advances_without_writing() {
        let mut data = vec![OP_GETWEATHER, 0x00];
        data.extend_from_slice(&REF0);
        data.push(OP_GETWEATHER);
        data.push(0x01);
        data.extend_from_slice(&REF0);
        data.extend_from_slice(&REF1);
        data.push(OP_END);
        let mut e = vm(data, vec![17, 100]);
        assert_eq!(e.step(), StepResult::Waiting);
        e.tick(1.0 / WAIT_UNITS_PER_SEC);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.work_zone(2), 0);
        assert_eq!(e.work_zone(3), 0);
        assert_eq!(e.work_zone(4), 0);
    }

    /// 0x72's undocumented sub-byte stops the VM rather than guessing a width
    /// (research/XiEvents/OpCodes/0x0072.md).
    #[test]
    fn getweather_unknown_sub_byte_stops() {
        let data = vec![OP_GETWEATHER, 0x02, OP_END];
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Unimplemented(OP_GETWEATHER));
    }

    /// 0x39 sets the event entity's facing from the raw work value
    /// (research/XiEvents/OpCodes/0x0039.md).
    #[test]
    fn set_facing_faces_the_event_entity() {
        assert_eq!(
            cues_of(OP_SET_FACING, &REF0, vec![2048]),
            [EventCue::ActorFace {
                actor: ActorLookup::EVENT_ENTITY,
                heading: 2048,
            }]
        );
    }

    /// 0x4B turns the named actor to the work-operand yaw
    /// (research/XiEvents/OpCodes/0x004B.md).
    #[test]
    fn yaw_turns_the_named_actor() {
        let mut operands = NPC_SERVER_ID.to_le_bytes().to_vec();
        operands.extend_from_slice(&REF0);
        assert_eq!(
            cues_of(OP_YAW, &operands, vec![100]),
            [EventCue::ActorFace {
                actor: ActorLookup(NPC_SERVER_ID),
                heading: 100,
            }]
        );
    }

    /// 0x36 places the event entity at the work-operand position, no heading
    /// (research/XiEvents/OpCodes/0x0036.md).
    #[test]
    fn set_event_pos_places_the_event_entity() {
        let mut operands = Vec::new();
        operands.extend_from_slice(&REF0);
        operands.extend_from_slice(&REF1);
        operands.extend_from_slice(&REF2);
        assert_eq!(
            cues_of(OP_SET_EVENT_POS, &operands, vec![20, 40, 3]),
            [EventCue::ActorPlace {
                actor: ActorLookup::EVENT_ENTITY,
                position: crate::vm::scene::EventPosition {
                    x: 20,
                    z: 40,
                    y: 3,
                    heading: 0,
                },
            }]
        );
    }

    /// 0xBA places the named actor at the work-operand position and heading
    /// (research/XiEvents/OpCodes/0x00BA.md).
    #[test]
    fn set_actor_pos_places_the_named_actor() {
        let mut operands = NPC_SERVER_ID.to_le_bytes().to_vec();
        operands.extend_from_slice(&REF0);
        operands.extend_from_slice(&REF1);
        operands.extend_from_slice(&REF2);
        operands.extend_from_slice(&REF3);
        assert_eq!(
            cues_of(OP_SET_ACTOR_POS, &operands, vec![20, 40, 3, 512]),
            [EventCue::ActorPlace {
                actor: ActorLookup(NPC_SERVER_ID),
                position: crate::vm::scene::EventPosition {
                    x: 20,
                    z: 40,
                    y: 3,
                    heading: 512,
                },
            }]
        );
    }

    /// 0x7D runs the work-operand scheduler on the local player with the player
    /// as its own target — the rank-up animations, tag `main`, no helper remap
    /// (research/XiEvents/OpCodes/0x007D.md).
    #[test]
    fn local_player_scheduler_runs_on_the_player() {
        const WORK: u32 = 100;
        assert_eq!(
            cues_of(OP_LOCAL_PLAYER_SCHEDULER, &REF0, vec![WORK]),
            [EventCue::Scheduler {
                dat_id: LOCAL_PLAYER_SCHEDULER_DAT_ID_BASE + WORK,
                actor1: ActorLookup::LOCAL_PLAYER,
                actor2: ActorLookup::LOCAL_PLAYER,
                tag: MAGIC_ROUTINE_TAG,
                duration: SCHEDULER_DURATION_FROM_DAT,
            }]
        );
    }

    /// A 0xC4 case past 2 arms no cast (retail's empty switch fall-through) yet
    /// still advances the full 12-byte width
    /// (research/XiEvents/OpCodes/0x00C4.md).
    #[test]
    fn magic_twin_0xc4_case_past_two_casts_nothing() {
        let mut data = vec![OP_MAGIC_TWIN, 0x03];
        data.extend_from_slice(&REF0);
        data.extend_from_slice(&[0x00, 0x00, 0x01]);
        data.push(0x00);
        data.extend_from_slice(&ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
        data.push(OP_END);
        let mut e = vm(data, vec![497]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), MAGIC_TWIN_SIZE);
        assert!(e.take_cues().is_empty());
    }

    /// 0x6E's work value splits into the emote id (low byte) and the variant
    /// selector (high byte); the cue names the actor the lookup operand picks
    /// (research/XiEvents/OpCodes/0x006E.md).
    #[test]
    fn emot_opcode_emits_the_split_emote_cue() {
        let mut operands = ActorLookup::LOCAL_PLAYER.0.to_le_bytes().to_vec();
        operands.extend_from_slice(&REF0);
        assert_eq!(
            cues_of(OP_EMOT, &operands, vec![0x0201]),
            [EventCue::Emote {
                actor: ActorLookup::LOCAL_PLAYER,
                emote_id: 1,
                param: 2,
            }]
        );
    }

    /// 0x63 emotes the event entity, reading the same work slot as 0x6E
    /// (research/XiEvents/OpCodes/0x0063.md).
    #[test]
    fn playanim_opcode_emotes_the_event_entity() {
        let operands = REF0.to_vec();
        assert_eq!(
            cues_of(OP_PLAYANIM, &operands, vec![0x0004]),
            [EventCue::Emote {
                actor: ActorLookup::EVENT_ENTITY,
                emote_id: 4,
                param: 0,
            }]
        );
    }

    /// A 0x99 in the same pass as its 0x6E parks on the same-pass start, the
    /// way retail's IsMovingAction sees the just-set animation
    /// (research/XiEvents/OpCodes/0x0099.md).
    #[test]
    fn animwait_parks_on_the_same_pass_emote() {
        let mut data = vec![OP_EMOT];
        data.extend_from_slice(&ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
        data.extend_from_slice(&REF0);
        data.push(OP_ANIMWAIT);
        data.extend_from_slice(&ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
        data.push(OP_END);
        let mut e = vm(data, vec![7]);
        assert_eq!(e.step(), StepResult::Waiting);
    }

    /// With no emote armed, a 0x99 falls through to the next opcode, retail's
    /// unresolved-entity path (research/XiEvents/OpCodes/0x0099.md).
    #[test]
    fn animwait_falls_through_with_no_armed_emote() {
        let mut data = vec![OP_ANIMWAIT];
        data.extend_from_slice(&ActorLookup::LOCAL_PLAYER.0.to_le_bytes());
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 5);
    }

    /// 0x2C's third operand is an ASCII action key, not a numeric id
    /// (research/XiEvents/OpCodes/0x002C.md).
    #[test]
    fn actor_motion_opcode_emits_its_ascii_action_key() {
        const KNEEL: FourCc = *b"kue0";
        let mut operands = NPC_SERVER_ID.to_le_bytes().to_vec();
        operands.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        operands.extend_from_slice(&KNEEL);
        assert_eq!(
            cues_of(OP_SCHEDULOR, &operands, vec![]),
            [EventCue::ActorMotion {
                actor1: ActorLookup(NPC_SERVER_ID),
                actor2: ActorLookup::EVENT_ENTITY,
                key: KNEEL,
            }]
        );
    }

    /// 0x4E's hide flag is bit 0 of the byte after the opcode; the target is the
    /// lookup at +2 and the cue is event-scoped either way
    /// (research/XiEvents/OpCodes/0x004E.md).
    #[test]
    fn event_hide_opcode_emits_both_directions() {
        for (flag, hide) in [(1u8, true), (0, false)] {
            let mut operands = vec![flag];
            operands.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
            assert_eq!(
                cues_of(OP_EVENTHIDE, &operands, vec![]),
                [EventCue::ActorHide {
                    target: ActorLookup(NPC_SERVER_ID),
                    hide,
                }]
            );
        }
    }

    /// `OP_EVENT_HIDE_SELF` carries no target operand: it hides/shows the
    /// event's own entity.
    #[test]
    fn event_hide_self_opcode_targets_the_event_entity() {
        for (flag, hide) in [(1u8, true), (0, false)] {
            assert_eq!(
                cues_of(OP_EVENT_HIDE_SELF, &[flag], vec![]),
                [EventCue::ActorHide {
                    target: ActorLookup::EVENT_ENTITY,
                    hide,
                }]
            );
        }
    }

    /// `OP_MAP_TUTORIAL`'s three work operands resolve through the References
    /// store; only the LOBYTE of the third is the tutorial flag.
    #[test]
    fn map_tutorial_opcode_resolves_its_work_operands() {
        let mut ops = vec![OP_MAP_TUTORIAL];
        for sel in [0x8001u16, 0x8002, 0x8003] {
            ops.extend_from_slice(&sel.to_le_bytes());
        }
        assert_eq!(
            cues_of(OP_MAP_TUTORIAL, &ops[1..], vec![0, 230, 0, 7]),
            [EventCue::MapOpen {
                map_id: 230,
                tutorial: true
            }]
        );
    }

    /// `OP_MAP_MARKER` carries the marker's zone id and milli-unit position in
    /// work slots and its 16-byte name inline; underscores become spaces.
    #[test]
    fn map_marker_opcode_carries_position_and_rewritten_name() {
        let mut ops = vec![OP_MAP_MARKER];
        for sel in [0x8001u16, 0x8002, 0x8003, 0x8004] {
            ops.extend_from_slice(&sel.to_le_bytes());
        }
        let mut name = [0u8; 16];
        name[..7].copy_from_slice(b"Ailevia");
        ops.extend_from_slice(&name);
        assert_eq!(
            cues_of(
                OP_MAP_MARKER,
                &ops[1..],
                vec![0, 230, 0, (-10264i32) as u32, (-363i32) as u32]
            ),
            [EventCue::MapMarker {
                map_id: 230,
                x_milli: -10264,
                y_milli: -363,
                name: *b"Ailevia\0\0\0\0\0\0\0\0\0",
            }]
        );

        let mut underscored = [0u8; 16];
        underscored[..9].copy_from_slice(b"some_name");
        let mut ops2 = vec![OP_MAP_MARKER];
        for sel in [0x8001u16, 0x8002, 0x8003, 0x8004] {
            ops2.extend_from_slice(&sel.to_le_bytes());
        }
        ops2.extend_from_slice(&underscored);
        assert_eq!(
            cues_of(
                OP_MAP_MARKER,
                &ops2[1..],
                vec![0, 230, 0, (-10264i32) as u32, (-363i32) as u32]
            ),
            [EventCue::MapMarker {
                map_id: 230,
                x_milli: -10264,
                y_milli: -363,
                name: *b"some name\0\0\0\0\0\0\0",
            }]
        );
    }

    /// `OP_OPEN_MAP` opens the map on the work-slot zone id, sub-menus
    /// hidden (research/XiEvents/OpCodes/0x0089.md).
    #[test]
    fn open_map_opcode_emits_the_open_cue() {
        assert_eq!(
            cues_of(OP_OPEN_MAP, &REF1, vec![0, 230]),
            [EventCue::MapOpen {
                map_id: 230,
                tutorial: false
            }]
        );
    }

    /// `OP_OPEN_MAP_PROPS`'s sub-menu property has no carrier field; the cue
    /// carries the zone id only (research/XiEvents/OpCodes/0x008D.md).
    #[test]
    fn open_map_props_opcode_drops_its_property_operand() {
        assert_eq!(
            cues_of(OP_OPEN_MAP_PROPS, &[REF1, REF2].concat(), vec![0, 230, 5]),
            [EventCue::MapOpen {
                map_id: 230,
                tutorial: false
            }]
        );
    }

    /// 0xD4 case 0 runs the 0x24 query helper and opens the current zone's map
    /// (type 6), parking on the answer; the message and default-cursor selectors
    /// sit at +2 and +4, where the embedded helper reads them
    /// (research/XiEvents/OpCodes/0x00D4.md).
    #[test]
    fn map_query_case0_opens_the_map_and_parks_on_the_answer() {
        let mut data = vec![OP_MAP_QUERY, 0x00];
        data.extend_from_slice(&REF0);
        data.extend_from_slice(&REF1);
        data.extend_from_slice(&[0x00, 0x00]);
        data.push(OP_END);
        let mut e = vm(data, vec![500, 0]);
        e.set_current_zone(283);
        assert_eq!(
            e.step(),
            StepResult::AwaitChoice(EventChoice {
                message_id: 500,
                speaker_index: 5,
                default_index: 0,
                params: vec![],
            })
        );
        let cues = e.take_cues();
        assert!(
            cues.contains(&EventCue::MapOpen {
                map_id: 283,
                tutorial: false
            }),
            "case 0 opens the current zone's map: {cues:?}"
        );
        e.select_choice(Some(1));
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 8, "case 0 advances its 8-byte width");
    }

    /// 0xD4 case 2 runs the 0x24 query helper without the map, parking on the
    /// answer (research/XiEvents/OpCodes/0x00D4.md).
    #[test]
    fn map_query_case2_parks_on_the_answer_without_the_map() {
        let mut data = vec![OP_MAP_QUERY, 0x02];
        data.extend_from_slice(&REF0);
        data.extend_from_slice(&REF1);
        data.extend_from_slice(&[0x00, 0x00]);
        data.push(OP_END);
        let mut e = vm(data, vec![500, 0]);
        e.set_current_zone(283);
        assert_eq!(
            e.step(),
            StepResult::AwaitChoice(EventChoice {
                message_id: 500,
                speaker_index: 5,
                default_index: 0,
                params: vec![],
            })
        );
        assert!(e.take_cues().is_empty(), "case 2 opens no map");
        e.select_choice(Some(1));
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 8);
    }

    /// 0xD4 cases 1/3/4/5 copy into the client's query window, which has no
    /// kuluu counterpart, so they advance by their width and emit no cue
    /// (research/XiEvents/OpCodes/0x00D4.md).
    #[test]
    fn map_query_data_cases_advance_by_their_width() {
        for (sub, width) in [(1u8, 8usize), (3, 6), (5, 12)] {
            let mut data = vec![OP_MAP_QUERY, sub];
            data.extend(std::iter::repeat_n(0, width - 2));
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            assert_eq!(e.step(), StepResult::Done, "case {sub}");
            assert_eq!(e.exec_pointer(), width, "case {sub} advances {width}");
            assert!(e.take_cues().is_empty(), "case {sub} emits no cue");
        }
    }

    /// 0xD4's unknown cases stop the VM: it is a jumping opcode, so the
    /// default arm's rule applies — no size-skip that would desync ExecPointer
    /// (research/XiEvents/OpCodes/0x00D4.md).
    #[test]
    fn map_query_unknown_case_stops() {
        let mut data = vec![OP_MAP_QUERY, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Unimplemented(OP_MAP_QUERY));
        assert_eq!(e.exec_pointer(), 0, "an unknown case does not advance");
    }

    /// 0xB3 RANKING: LSB has no ranking handler, so every case advances by
    /// its width and emits no cue (research/XiEvents/OpCodes/0x00B3.md).
    #[test]
    fn ranking_cases_advance_by_their_widths() {
        for (sub, width) in [
            (0u8, 4usize),
            (1, 14),
            (2, 2),
            (3, 4),
            (4, 4),
            (5, 18),
            (6, 4),
            (7, 4),
            (8, 2),
            (9, 4),
            (0x0A, 2),
        ] {
            let mut data = vec![OP_RANKING, sub];
            data.extend(std::iter::repeat_n(0, width - 2));
            data.push(OP_END);
            let mut e = vm(data, vec![]);
            assert_eq!(e.step(), StepResult::Done, "case {sub}");
            assert_eq!(e.exec_pointer(), width, "case {sub} advances {width}");
            assert!(e.take_cues().is_empty(), "case {sub} emits no cue");
        }
    }

    /// 0xB3's read cases fill the board's work slots from the server answer;
    /// with no ranking server to answer, they write zeros so the board draws
    /// empty instead of the event dying
    /// (research/XiEvents/OpCodes/0x00B3.md).
    #[test]
    fn ranking_read_cases_zero_their_board_slots() {
        let sel = |slot: u32| -> [u8; 2] { ((WORK_ZONE_BASE + slot) as u16).to_le_bytes() };
        for (sub, width, slots, untouched) in [
            (1u8, 14usize, &[2u32, 4, 6, 8, 2, 4][..], Some(3u32)),
            (5, 18, &[2, 3, 4, 5, 6, 7, 8, 9][..], Some(10)),
            (9, 4, &[2][..], Some(3)),
        ] {
            let mut data = vec![OP_RANKING, sub];
            for &slot in slots {
                data.extend_from_slice(&sel(slot));
            }
            data.extend(std::iter::repeat_n(0, width - 2 - 2 * slots.len()));
            data.push(OP_END);
            let mut e = EventVm::start(&block(data, vec![]), 7, 5, vec![1; 8]).unwrap();
            if let Some(slot) = untouched {
                if slot < 10 {
                    assert_eq!(e.work_zone(slot as usize), 1, "params pre-set slot {slot}");
                }
            }
            assert_eq!(e.step(), StepResult::Done, "case {sub}");
            assert_eq!(e.exec_pointer(), width, "case {sub} advances {width}");
            for &slot in slots {
                assert_eq!(
                    e.work_zone(slot as usize),
                    0,
                    "case {sub} zeroes slot {slot}"
                );
            }
            if let Some(slot) = untouched {
                assert_eq!(
                    e.work_zone(slot as usize),
                    if slot < 10 { 1 } else { 0 },
                    "case {sub} leaves untouched slot {slot} alone"
                );
            }
        }
    }

    /// `OP_MAP_ADD_MARK` carries the marker's zone id and milli-unit position
    /// in work slots, its sub-menu and index operands uncarried, and its
    /// 16-byte name inline with the underscore rewrite
    /// (research/XiEvents/OpCodes/0x00B8.md).
    #[test]
    fn map_add_mark_opcode_carries_position_and_rewritten_name() {
        let mut ops = [REF1, REF2, REF3].concat();
        ops.extend_from_slice(&[4, 0x80]);
        ops.extend_from_slice(&[5, 0x80]);
        let mut name = [0u8; 16];
        name[..9].copy_from_slice(b"some_name");
        ops.extend_from_slice(&name);
        assert_eq!(
            cues_of(
                OP_MAP_ADD_MARK,
                &ops,
                vec![0, 230, 0, 3, (-10264i32) as u32, (-363i32) as u32]
            ),
            [EventCue::MapMarker {
                map_id: 230,
                x_milli: -10264,
                y_milli: -363,
                name: *b"some name\0\0\0\0\0\0\0",
            }]
        );
    }

    #[test]
    fn close_map_opcode_emits_the_close_cue() {
        assert_eq!(cues_of(OP_CLOSE_MAP, &[], vec![]), [EventCue::MapClose]);
    }

    /// Event 503's epilogue beat (master +045D9..+04606): MAP_TUTORIAL,
    /// MAP_MARKER, the coupon line, MESWAIT, CLOSE_MAP. Retail parks at MESWAIT
    /// with the map window still open — 0x8A only runs after the player answers
    /// (research/XiEvents/OpCodes/0x0023.md: RetFlag up until dismissal).
    #[test]
    fn close_map_holds_until_the_player_answers_the_line() {
        let mut data = vec![OP_MAP_TUTORIAL];
        for sel in [0x8001u16, 0x8002, 0x8003] {
            data.extend_from_slice(&sel.to_le_bytes());
        }
        data.push(OP_MESSAGE);
        data.extend_from_slice(&REF0);
        data.push(OP_MESWAIT);
        data.push(OP_CLOSE_MAP);
        data.push(OP_END);
        let mut e = vm(data, vec![900, 230, 0, 7]);
        assert_eq!(
            e.step(),
            StepResult::AwaitMessage(EventMessage {
                message_id: 900,
                speaker_index: Some(5),
                params: vec![],
            })
        );
        let cues = e.take_cues();
        assert!(
            cues.contains(&EventCue::MapOpen {
                map_id: 230,
                tutorial: true
            }),
            "the beat opens the map before its line"
        );
        assert!(
            !cues.iter().any(|c| matches!(c, EventCue::MapClose)),
            "CLOSE_MAP must not run while the line is up: {cues:?}"
        );
        // Re-step stays parked on MESWAIT (retail yields every tick with the
        // open flag up); the map still does not close
        // (research/XiEvents/OpCodes/0x0023.md).
        assert_eq!(e.step(), StepResult::Waiting);
        assert!(!e
            .take_cues()
            .iter()
            .any(|c| matches!(c, EventCue::MapClose)));
        e.dismiss_message();
        assert_eq!(e.step(), StepResult::Done);
        let cues = e.take_cues();
        assert!(
            cues.contains(&EventCue::MapClose),
            "CLOSE_MAP runs only after dismissal: {cues:?}"
        );
    }

    /// `OP_HIDE_HUD`/`OP_SHOW_HUD` stage the whole-HUD hide/show; the hide
    /// operands are retail's event-message presets, so they carry no cue
    /// payload.
    #[test]
    fn hud_opcodes_emit_the_hide_and_show_cues() {
        // Southern San d'Oria event 503 authors these operand bytes
        // (research/XiEvents/OpCodes/0x0067.md).
        assert_eq!(
            cues_of(OP_HIDE_HUD, &[0x91, 0x80, 0xA7, 0x81], vec![]),
            [EventCue::HudHide { hide: true }]
        );
        assert_eq!(
            cues_of(OP_SHOW_HUD, &[], vec![]),
            [EventCue::HudHide { hide: false }]
        );
    }

    /// `OP_STOP_CLOCK`'s hour operand resolves through References and wraps
    /// the Vana'diel day; the sentinel means no time change. Hour -> refs[1],
    /// weather -> refs[3] (the latter has no cue).
    #[test]
    fn stop_clock_opcode_carries_the_authored_hour() {
        let ops = [0x01u8, 0x80, 0x03, 0x80];
        assert_eq!(
            cues_of(OP_STOP_CLOCK, &ops, vec![0, 8, 0, 1]),
            [EventCue::ClockHold {
                stop: true,
                hour: Some(8),
                minute: 0,
                day_from_epoch: None
            }]
        );
        assert_eq!(
            cues_of(OP_STOP_CLOCK, &ops, vec![0, 30, 0, 1]),
            [EventCue::ClockHold {
                stop: true,
                hour: Some(6),
                minute: 0,
                day_from_epoch: None
            }]
        );
        assert!(cues_of(OP_STOP_CLOCK, &ops, vec![0, 255, 0, 1]).is_empty());
    }

    #[test]
    fn restore_clock_opcode_releases_the_hold() {
        assert_eq!(
            cues_of(OP_RESTORE_CLOCK, &[], vec![]),
            [EventCue::ClockHold {
                stop: false,
                hour: None,
                minute: 0,
                day_from_epoch: None
            }]
        );
    }

    /// 0xA9's day operand is multiplied by seven: work slot 1 of 2 lands on
    /// Vana day 14 at 00:30 (research/XiEvents/OpCodes/0x00A9.md).
    #[test]
    fn set_clock_date_opcode_jumps_seven_days_per_work_unit() {
        let ops = [0x01u8, 0x80];
        assert_eq!(
            cues_of(OP_SET_CLOCK_DATE, &ops, vec![0, 2]),
            [EventCue::ClockHold {
                stop: true,
                hour: Some(0),
                minute: 30,
                day_from_epoch: Some(14)
            }]
        );
        assert_eq!(
            cues_of(OP_SET_CLOCK_DATE, &ops, vec![0, 0]),
            [EventCue::ClockHold {
                stop: true,
                hour: Some(0),
                minute: 30,
                day_from_epoch: Some(0)
            }]
        );
    }

    /// 0xC9 releases the game-timer hold, the same cue 0x78 emits
    /// (research/XiEvents/OpCodes/0x00C9.md, 0x0078.md).
    #[test]
    fn enable_timer_opcode_releases_the_hold() {
        assert_eq!(
            cues_of(OP_ENABLE_TIMER, &[], vec![]),
            [EventCue::ClockHold {
                stop: false,
                hour: None,
                minute: 0,
                day_from_epoch: None
            }]
        );
    }

    /// 0x69's flag byte selects full volume (0) or mute (nonzero); the mask
    /// rides work slot 2 (research/XiEvents/OpCodes/0x0069.md).
    #[test]
    fn set_sound_volume_opcode_toggles_the_named_channels() {
        let ops = [0x00, 0x02, 0x80];
        assert_eq!(
            cues_of(OP_SET_SOUND_VOLUME, &ops, vec![0, 0, 0x04]),
            [EventCue::SoundVolume {
                mask: 0x04,
                volume: MUSIC_VOLUME_MAX,
                fade_frames: 0
            }]
        );
        let mute = [0x01, 0x02, 0x80];
        assert_eq!(
            cues_of(OP_SET_SOUND_VOLUME, &mute, vec![0, 0, 0x08]),
            [EventCue::SoundVolume {
                mask: 0x08,
                volume: 0,
                fade_frames: 0
            }]
        );
    }

    /// 0x6A scales work[1] by 0.001 onto the 0..=127 table, carries work[3]
    /// as the fade and work[5] as the mask (research/XiEvents/OpCodes/0x006A.md).
    #[test]
    fn change_sound_volume_opcode_scales_the_level_and_carries_the_fade() {
        let ops = [0x01u8, 0x80, 0x03, 0x80, 0x05, 0x80];
        assert_eq!(
            cues_of(OP_CHANGE_SOUND_VOLUME, &ops, vec![0, 500, 0, 60, 0, 0x04]),
            [EventCue::SoundVolume {
                mask: 0x04,
                volume: 63,
                fade_frames: 60
            }]
        );
        assert_eq!(
            cues_of(OP_CHANGE_SOUND_VOLUME, &ops, vec![0, 9999, 0, 0, 0, 0x01]),
            [EventCue::SoundVolume {
                mask: 0x01,
                volume: MUSIC_VOLUME_MAX,
                fade_frames: 0
            }]
        );
    }

    /// `OP_DEFCAMERA` case 1 takes the camera, case 0 gives it back; case 2
    /// only queries the current state, so it stages nothing.
    #[test]
    fn camera_opcode_emits_only_its_lock_and_unlock_cases() {
        assert_eq!(
            cues_of(OP_DEFCAMERA, &[DEFCAMERA_CASE_LOCK], vec![]),
            [EventCue::CameraLock { lock: true }]
        );
        assert_eq!(
            cues_of(OP_DEFCAMERA, &[DEFCAMERA_CASE_UNLOCK], vec![]),
            [EventCue::CameraLock { lock: false }]
        );
        assert!(cues_of(OP_DEFCAMERA, &[2, 0x0A, 0x00], vec![]).is_empty());
    }

    /// 0x38 writes the operand's high byte with 0x20 forced: retail's Ailevia
    /// tour stores 0x2003, which applies as 0x20, as does the 0x0013 default
    /// that covers most of the retail corpus
    /// (research/XiEvents/OpCodes/0x0038.md).
    #[test]
    fn local_mode_opcode_carries_the_high_byte_with_the_cinematic_bit() {
        for (stored, applied) in [
            (0x2003u32, 0x20u16),
            (0x0013, 0x20),
            (0x0413, 0x24),
            (0, 0x20),
        ] {
            assert_eq!(
                cues_of(OP_LOCAL_MODE, &[0x01, 0x80], vec![0, stored]),
                [EventCue::LocalMode { mode: applied }]
            );
        }
    }

    /// 0x20 writes retail's CliEventUcFlag: any nonzero byte locks the
    /// player, zero releases it (research/XiEvents/OpCodes/0x0020.md).
    #[test]
    fn player_control_opcode_writes_the_flag() {
        assert_eq!(
            cues_of(OP_PLAYER_CONTROL, &[1], vec![]),
            [EventCue::PlayerControl { locked: true }]
        );
        assert_eq!(
            cues_of(OP_PLAYER_CONTROL, &[0], vec![]),
            [EventCue::PlayerControl { locked: false }]
        );
    }

    /// `OP_MUSICVOLUME`'s first operand is a volume *table index*, its second
    /// a frame count.
    #[test]
    fn music_volume_opcode_emits_table_index_and_frame_count() {
        const DUCKED: u32 = 32;
        const FADE_FRAMES: u32 = 120;
        let operands = [0x00, 0x80, 0x01, 0x80];
        assert_eq!(
            cues_of(OP_MUSICVOLUME, &operands, vec![DUCKED, FADE_FRAMES]),
            [EventCue::MusicVolume {
                volume: DUCKED as u8,
                fade_frames: FADE_FRAMES as u16,
            }]
        );
        // The table tops out at MUSIC_VOLUME_MAX, so an out-of-range work value
        // saturates rather than wrapping into a quiet volume.
        assert_eq!(
            cues_of(OP_MUSICVOLUME, &operands, vec![9999, 0]),
            [EventCue::MusicVolume {
                volume: MUSIC_VOLUME_MAX,
                fade_frames: 0,
            }]
        );
    }

    /// 0x5C's low band sets the BGM slot's song from the +2 work selector and
    /// starts it at full volume; the 0x80 band names the slot in its high
    /// nibble and carries the start volume at +4; 0xA0/0xA1 ride 0x5D's shape
    /// (research/XiEvents/OpCodes/0x005C.md).
    #[test]
    fn music_opcode_sets_the_slot_song_and_start_volume() {
        let low = [0x00u8, 0x01, 0x80];
        assert_eq!(
            cues_of(OP_MUSIC, &low, vec![0, 101]),
            [EventCue::MusicSong {
                slot: 0,
                track: 101,
                volume: MUSIC_VOLUME_MAX
            }]
        );
        let vol = [0x83u8, 0x01, 0x80, 0x02, 0x80];
        assert_eq!(
            cues_of(OP_MUSIC, &vol, vec![0, 99, 40]),
            [EventCue::MusicSong {
                slot: 3,
                track: 99,
                volume: 40
            }]
        );
        let fade = [0xA0u8, 0x01, 0x80, 0x02, 0x80];
        assert_eq!(
            cues_of(OP_MUSIC, &fade, vec![0, 32, 60]),
            [EventCue::MusicVolume {
                volume: 32,
                fade_frames: 60
            }]
        );
    }

    /// 0x7E's mount cases, and the one that must not stage anything: case 2
    /// re-runs in retail until a mount attachment reports ready, a signal this
    /// VM has no source for, so it advances silently instead of spinning.
    #[test]
    fn mount_opcode_emits_per_case_status_events() {
        let player = ActorLookup::LOCAL_PLAYER.0.to_le_bytes();
        let case = |sub: u8| {
            let mut o = vec![sub];
            o.extend_from_slice(&player);
            o
        };
        let mount = |status_event, mount_id| {
            [EventCue::Mount {
                target: ActorLookup::LOCAL_PLAYER,
                status_event,
                mount_id,
            }]
        };
        assert_eq!(
            cues_of(OP_CHOCOBO, &case(1), vec![]),
            mount(STATUS_EVENT_CHOCOBO, None)
        );
        for sub in CHOCOBO_CASES_IDLE {
            assert_eq!(
                cues_of(OP_CHOCOBO, &case(sub), vec![]),
                mount(STATUS_EVENT_IDLE, None),
                "0x7E case {sub}"
            );
        }
        assert!(cues_of(OP_CHOCOBO, &case(2), vec![]).is_empty());

        // Case 7's mount id is its work operand biased by one.
        const MOUNT_WORK: u32 = 3;
        let mut seven = case(CHOCOBO_CASE_MOUNT);
        seven.extend_from_slice(&REF0);
        assert_eq!(
            cues_of(OP_CHOCOBO, &seven, vec![MOUNT_WORK]),
            mount(STATUS_EVENT_MOUNT, Some(MOUNT_WORK as u16 + 1))
        );
        assert_eq!(
            cues_of(OP_CHOCOBO, &case(CHOCOBO_CASE_UNMOUNT), vec![]),
            mount(STATUS_EVENT_IDLE, Some(CHOCOBO_UNMOUNT_ID))
        );
    }

    /// 0x4C/0x4D/0x4F/0x8E/0x8F write the event entity's StatusEvent on the
    /// Mount cue: the door's open/close byte, its D_OPEN2/D_CLOSE2 pair, and
    /// work(1) + 18 into the M1..M8 range
    /// (research/XiEvents/OpCodes/0x004C.md, 0x004D.md, 0x004F.md,
    /// 0x008E.md, 0x008F.md).
    #[test]
    fn door_status_opcodes_write_the_event_entity_status() {
        let door = |status_event| {
            [EventCue::Mount {
                target: ActorLookup::EVENT_ENTITY,
                status_event,
                mount_id: None,
            }]
        };
        assert_eq!(
            cues_of(OP_DOOR_OPEN, &[], vec![]),
            door(STATUS_EVENT_DOOR_OPEN)
        );
        assert_eq!(
            cues_of(OP_DOOR_CLOSE, &[], vec![]),
            door(STATUS_EVENT_DOOR_CLOSE)
        );
        assert_eq!(
            cues_of(OP_DOOR_OPEN2, &[], vec![]),
            door(STATUS_EVENT_DOOR_OPEN2)
        );
        assert_eq!(
            cues_of(OP_DOOR_CLOSE2, &[], vec![]),
            door(STATUS_EVENT_DOOR_CLOSE2)
        );
        assert_eq!(
            cues_of(OP_STATUS_EVENT, &REF1, vec![0, 3]),
            door(STATUS_EVENT_MOTION_BASE as u8 + 3)
        );
    }

    /// 0x90 hides the event entity: the 0x4E bit with the value fixed at 1
    /// (research/XiEvents/OpCodes/0x0090.md, 0x004E.md).
    #[test]
    fn event_hide_always_opcode_hides_the_event_entity() {
        assert_eq!(
            cues_of(OP_EVENT_HIDE_ALWAYS, &[], vec![]),
            [EventCue::ActorHide {
                target: ActorLookup::EVENT_ENTITY,
                hide: true,
            }]
        );
    }

    /// Cues accumulate across a step in execution order and drain exactly once.
    #[test]
    fn take_cues_drains_in_execution_order() {
        let mut data = vec![OP_DEFCAMERA, DEFCAMERA_CASE_LOCK, OP_EVENTHIDE, 1];
        data.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(
            e.take_cues(),
            [
                EventCue::CameraLock { lock: true },
                EventCue::ActorHide {
                    target: ActorLookup(NPC_SERVER_ID),
                    hide: true,
                },
            ]
        );
        assert!(e.take_cues().is_empty(), "a drained cue is not replayed");
    }

    /// The send-tag pair: case 0 sends the tag with EndPara = Work_Zone[1],
    /// which the program seeds from refs[1], and holds execution on the
    /// following case-1 poll; ack_server releases it past the pair
    /// (research/XiEvents/OpCodes/0x0043.md).
    #[test]
    fn sendtag_pair_holds_until_ack() {
        let mut data = vec![OP_GET_STORE, 0x01, 0x10, SRC[0], SRC[1]];
        data.extend_from_slice(&[OP_SENDTAG, 0x00, OP_SENDTAG, 0x01, OP_END]);
        let mut e = vm(data, vec![0, 42]);

        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::SendTag { end_para: 42 })
        );
        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::SendTag { end_para: 42 }),
            "a second step must not resend"
        );

        e.ack_server();
        assert_eq!(e.step(), StepResult::Done);
    }

    /// A bare case-1 poll of the send-tag opcode with no outstanding tag takes
    /// retail's acknowledged path: skip past and run on.
    #[test]
    fn sendtag_poll_without_pending_tag_skips() {
        let data = vec![OP_SENDTAG, 0x01, OP_END];
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
    }

    /// 0xA7 pair: case 0 arms the await and sends the tag (EndPara =
    /// Work_Zone[1], which the program seeds from refs[1]), holding until the
    /// server acks; case 1 then writes the result the ack carried into the
    /// work slot its +2 operand selects (research/XiEvents/OpCodes/0x00A7.md).
    #[test]
    fn a7_pair_sends_the_tag_and_writes_the_result() {
        let mut data = vec![OP_GET_STORE, 0x01, 0x10, SRC[0], SRC[1]];
        data.extend_from_slice(&[OP_A7_WAIT, 0x00]);
        data.extend_from_slice(&[OP_A7_WAIT, 0x01, 0x03, 0x10]);
        data.push(OP_END);
        let mut e = vm(data, vec![0, 42]);

        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::SendTag { end_para: 42 })
        );
        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::SendTag { end_para: 42 }),
            "a second step must not resend"
        );

        e.ack_server();
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(
            e.work_zone(3),
            42,
            "case 1 writes the result the ack carried"
        );
    }

    /// A bare 0xA7 case-1 poll with no outstanding tag writes the zero result
    /// and advances its width, matching retail's acknowledged path
    /// (research/XiEvents/OpCodes/0x00A7.md).
    #[test]
    fn a7_case1_without_a_tag_writes_the_zero_result() {
        let data = vec![OP_A7_WAIT, 0x01, 0x03, 0x10, OP_END];
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 4, "case 1 advances its 4-byte width");
        assert_eq!(e.work_zone(3), 0, "no tag means the zero result");
    }

    /// 0xA6 pair: case 0 sends the 0x0EB request and holds on the 0x10E
    /// answer, case 1 yields until it lands, and case 2 writes the answered
    /// MapNum into the work slot its +2 operand selects
    /// (research/XiEvents/OpCodes/0x00A6.md).
    #[test]
    fn a6_pair_requests_and_writes_the_submap_num() {
        let mut data = vec![OP_A6_SUBMAP, 0x00, OP_A6_SUBMAP, 0x01, OP_A6_SUBMAP, 0x02];
        data.extend_from_slice(&DST);
        data.push(OP_END);
        let mut e = vm(data, vec![]);

        assert_eq!(e.step(), StepResult::AwaitServerAck(PendingTag::SubMapNum));
        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::SubMapNum),
            "a second step must not resend"
        );

        e.set_submap_num(77);
        e.ack_server();
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.work_zone(0), 77, "case 2 writes the answered MapNum");
    }

    /// A bare 0xA6 case-2 read with no outstanding request writes the zero
    /// MapNum and advances its width, like retail's unanswered path
    /// (research/XiEvents/OpCodes/0x00A6.md).
    #[test]
    fn a6_case2_without_a_request_writes_the_zero_result() {
        let mut data = vec![OP_A6_SUBMAP, 0x02];
        data.extend_from_slice(&DST);
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 4, "case 2 advances its 4-byte width");
        assert_eq!(e.work_zone(0), 0, "no answer means the zero MapNum");
    }

    /// 0x87/0x88 pair: each send case arms the 0x01B FRIENDPASS with its
    /// case's `Para` (0x87: 0 begin / 2 begin-gold; 0x88: 1 confirm /
    /// 3 confirm-gold), holds on the 0x059 answer, and case 1 yields until it
    /// lands (research/XiEvents/OpCodes/0x0087.md, 0x0088.md).
    #[test]
    fn friendpass_pairs_arm_their_para_and_hold() {
        for (op, paras) in [(OP_FRIENDPASS_87, [0u16, 2]), (OP_FRIENDPASS_88, [1, 3])] {
            for (sub, para) in [(0u8, paras[0]), (2, paras[1])] {
                let data = vec![op, sub, op, 0x01, OP_END];
                let mut e = vm(data, vec![]);
                assert_eq!(
                    e.step(),
                    StepResult::AwaitServerAck(PendingTag::FriendPass { para }),
                    "0x{op:02X} sub {sub} arms para {para}"
                );
                assert_eq!(
                    e.step(),
                    StepResult::AwaitServerAck(PendingTag::FriendPass { para }),
                    "a second step must not resend"
                );
                e.ack_server();
                assert_eq!(e.step(), StepResult::Done);
            }
        }
    }

    /// Seed `slot` of Work_Zone from `refs[i]` with one GET_STORE each.
    fn seed_work_slots(slots: &[(u16, u16)]) -> (Vec<u8>, Vec<u32>) {
        let mut data = Vec::new();
        let mut refs = vec![0u32];
        for (slot, value) in slots {
            refs.push(u32::from(*value));
            data.push(OP_GET_STORE);
            data.extend_from_slice(&(*slot + WORK_ZONE_BASE as u16).to_le_bytes());
            data.extend_from_slice(
                &((refs.len() - 1) as u16 | REFERENCE_FLAG as u16).to_le_bytes(),
            );
        }
        (data, refs)
    }

    /// 0x8C case 0 arms the 0x058 RECIPE with Mode 1 and the work-slot fields
    /// its retail pseudo code names (skill work[2], level work[4], Param0
    /// work[6]), holds on the 0x031 answer, and case 1 yields until it lands
    /// (research/XiEvents/OpCodes/0x008C.md).
    #[test]
    fn recipe_case0_arms_mode1_from_the_work_slots() {
        let (seed, refs) = seed_work_slots(&[(2, 3), (4, 40), (6, 7)]);
        let mut data = seed;
        data.extend_from_slice(&[OP_RECIPE, 0x00, 0x02, 0x10, 0x04, 0x10, 0x06, 0x10]);
        data.extend_from_slice(&[OP_RECIPE, 0x01, OP_END]);
        let mut e = vm(data, refs);
        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::Recipe {
                mode: 1,
                skill: 3,
                level: 40,
                param0: 7,
                param1: 0,
                param2: 0,
                param3: 0,
                param4: 0,
            })
        );
        e.ack_server();
        assert_eq!(e.step(), StepResult::Done);
    }

    /// 0x8C cases 2/3/4 arm Mode 2/3/4 with their own work-slot field maps
    /// (research/XiEvents/OpCodes/0x008C.md).
    #[test]
    fn recipe_cases_fill_their_mode_and_params() {
        let (seed, refs) = seed_work_slots(&[(2, 3), (4, 40), (6, 7), (8, 11), (10, 13)]);
        let cases: [(u8, &[u8], PendingTag); 3] = [
            (
                2u8,
                &[0x02, 0x10, 0x04, 0x10, 0x06, 0x10, 0x08, 0x10, 0x0A, 0x10],
                PendingTag::Recipe {
                    mode: 2,
                    skill: 3,
                    level: 40,
                    param0: 0,
                    param1: 11,
                    param2: 13,
                    param3: 0,
                    param4: 7,
                },
            ),
            (
                3,
                &[0x02, 0x10, 0x04, 0x10, 0x06, 0x10, 0x08, 0x10],
                PendingTag::Recipe {
                    mode: 3,
                    skill: 3,
                    level: 40,
                    param0: 0,
                    param1: 0,
                    param2: 0,
                    param3: 11,
                    param4: 7,
                },
            ),
            (
                4,
                &[0x02, 0x10, 0x04, 0x10, 0x06, 0x10, 0x08, 0x10],
                PendingTag::Recipe {
                    mode: 4,
                    skill: 3,
                    level: 40,
                    param0: 0,
                    param1: 0,
                    param2: 0,
                    param3: 11,
                    param4: 7,
                },
            ),
        ];
        for (sub, selectors, tag) in cases {
            let mut data = seed.clone();
            data.push(OP_RECIPE);
            data.push(sub);
            data.extend_from_slice(selectors);
            data.extend_from_slice(&[OP_RECIPE, 0x01, OP_END]);
            let mut e = vm(data, refs.clone());
            assert_eq!(
                e.step(),
                StepResult::AwaitServerAck(tag.clone()),
                "case {sub}"
            );
            e.ack_server();
            assert_eq!(e.step(), StepResult::Done);
        }
    }

    /// 0x8C case 5 advances its 14-byte width without arming a tag: retail
    /// sends mode 5 without setting RecRecipeFlag, and LSB implements no
    /// mode 5 (research/XiEvents/OpCodes/0x008C.md).
    #[test]
    fn recipe_case5_advances_without_a_tag() {
        let mut data = vec![OP_RECIPE, 0x05];
        data.extend_from_slice(&[0u8; 12]);
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 14, "case 5 advances its 14-byte width");
        assert_eq!(e.pending_tag(), None);
    }

    /// 0xB2 mode 0 counts down WaitTime by frame delay and steps its 4-byte
    /// width on expiry. Retail reads WaitTime as the u16 at +1, which spans
    /// the mode byte and the first operand byte, so the authored events steer
    /// that read to work_zone[0] with a 0x10 first byte; the program seeds
    /// work_zone[0] from refs[1] (60 units = one second)
    /// (research/XiEvents/OpCodes/0x00B2.md).
    #[test]
    fn b2_mode0_waits_then_advances() {
        let mut data = vec![OP_GET_STORE, DST[0], DST[1], SRC[0], SRC[1]];
        data.extend_from_slice(&[OP_B2_DELIVERY, 0x00, 0x10, 0x00, OP_END]);
        let mut e = vm(data, vec![0, 60]);
        assert_eq!(e.step(), StepResult::Waiting);
        e.tick(0.5);
        assert_eq!(e.step(), StepResult::Waiting, "halfway through the wait");
        e.tick(0.51);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 9, "mode 0 advances its 4-byte width");
    }

    /// 0xB2 mode 1 arms the 0x04D PBX open request and holds until the server
    /// acks it, then steps its 2-byte width
    /// (research/XiEvents/OpCodes/0x00B2.md).
    #[test]
    fn b2_mode1_holds_until_the_delivery_ack() {
        let data = vec![OP_B2_DELIVERY, 0x01, OP_END];
        let mut e = vm(data, vec![]);
        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::DeliveryOpen)
        );
        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::DeliveryOpen),
            "a second step must not resend"
        );
        e.ack_server();
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 2, "mode 1 advances its 2-byte width");
    }

    /// The position-tag pair: case 0 scales its work-slot raw values into the
    /// c2s payload (coordinates times 0.001; heading through LSB's
    /// radianToRotation scale) and holds on the following case-1 poll until
    /// ack_server releases it (research/XiEvents/OpCodes/0x0047.md).
    /// refs[1..5] hold the raw work values: x, y, z, dir (1/4096-turn units);
    /// each is a refs[i] operand, the index with the REFERENCE_FLAG marker
    /// byte. A quarter turn lands at 63.998 on the wire's 0..=255 scale:
    /// retail's f32 literals fall just short of exactly a quarter and the
    /// conversion truncates rather than rounds.
    #[test]
    fn xzy_tag_pair_scales_work_slots_and_holds_until_ack() {
        let mut data = vec![OP_EVENTPOSSET, 0x00];
        for i in 1u16..=4 {
            data.extend_from_slice(&[i as u8, (REFERENCE_FLAG >> 8) as u8]);
        }
        data.extend_from_slice(&[OP_EVENTPOSSET, 0x01, OP_END]);
        let mut e = vm(data, vec![0, 2048, (-512i32) as u32, 4096, 1024]);

        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::SendXzy {
                x: 2.048,
                y: -0.512,
                z: 4.096,
                dir: 63,
                end_para: 0,
            })
        );

        e.ack_server();
        assert_eq!(e.step(), StepResult::Done);
    }

    /// PENDINGNUM's num[8] lands in Work_Zone from index 2 and a later read
    /// sees it (research/XiPackets/world/server/0x005C).
    #[test]
    fn pending_num_writes_work_zone_from_index_two() {
        let mut e = vm(vec![OP_END], vec![]);
        e.apply_pending_num(&[7, 8, 9, 10, 11, 12, 13, 14]);
        for (index, value) in (2..10).zip([7, 8, 9, 10, 11, 12, 13, 14]) {
            assert_eq!(e.work_zone(index), value);
        }
        assert_eq!(e.work_zone(0), 0);
        assert_eq!(e.work_zone(1), 0);
    }

    /// The full pending-tag round trip as the signet scripts use it: case 0
    /// sends the tag and holds; s2c PENDINGNUM updates Work_Zone[2] while still
    /// held; ack_server releases past the pair, and the script's loop test reads
    /// the updated value (research/XiPackets/world/server/0x005C). Layout:
    /// [0..5) WZ[1] = refs[0]; [5..7) send-tag send+hold; [7..9) poll; [9..17)
    /// IF case 1: jump when WZ[2] == refs[1], target absolute 20; [17..20)
    /// filler; [20..23) WZ[3] = 1; [23] END.
    #[test]
    fn pending_num_lands_while_held_and_ack_runs_on() {
        let mut data = vec![OP_GET_STORE, 0x01, 0x10, REF0[0], REF0[1]];
        data.extend_from_slice(&[OP_SENDTAG, 0x00, OP_SENDTAG, 0x01]);
        data.extend_from_slice(&[OP_IF, 2, 0x10, SRC[0], SRC[1], 1, 20, 0]);
        data.extend_from_slice(&[POISON; 3]);
        data.extend_from_slice(&[OP_SET_ONE, 0x03, 0x10]);
        data.push(OP_END);
        let mut e = vm(data, vec![42, 7]);

        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::SendTag { end_para: 42 })
        );
        let mut num = [0i32; 8];
        num[0] = 7;
        e.apply_pending_num(&num);
        assert_eq!(
            e.work_zone(2),
            7,
            "the loop condition must see it before the next step"
        );

        e.ack_server();
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 23, "must land on END past the SET_ONE");
        assert_eq!(
            e.work_zone(3),
            1,
            "the branch must have taken the updated value"
        );
    }

    /// A bare case-1 poll of the position-tag opcode with no outstanding tag
    /// skips past, like its send-tag twin.
    #[test]
    fn xzy_tag_poll_without_pending_tag_skips() {
        let data = vec![OP_EVENTPOSSET, 0x01, OP_END];
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
    }

    // 0x1F MOVE and 0x32 MainSpeed are owned by scene.rs when a scene exists;
    // these literals keep the fixture bytecode readable without importing them.
    const OP_MOVE_TEST: u8 = 0x1F;
    const OP_SPEED_TEST: u8 = 0x32;

    /// A zone DAT with the master block (event 7 at offset 0) and one NPC
    /// block whose tag index `tag` starts at offset 0 of its own bytecode.
    fn scene_dat(
        master: Vec<u8>,
        npc_data: Vec<u8>,
        npc_refs: Vec<u32>,
    ) -> std::sync::Arc<ffxi_dat::event_dat::EventDat> {
        let mut dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![block(master, vec![])],
        };
        dat.blocks.push(EventBlock {
            actor: NPC_SERVER_ID,
            event_ids: vec![7, 7],
            event_offsets: vec![0, 0],
            references: npc_refs,
            event_data: npc_data,
        });
        std::sync::Arc::new(dat)
    }

    fn scene_vm(master: Vec<u8>, npc_data: Vec<u8>) -> EventVm {
        scene_vm_refs(master, npc_data, vec![])
    }

    /// [`scene_vm`] with the NPC block's references table set.
    fn scene_vm_refs(master: Vec<u8>, npc_data: Vec<u8>, npc_refs: Vec<u32>) -> EventVm {
        let mut e = vm(master.clone(), vec![]);
        e.attach_scene(
            scene_dat(master, npc_data, npc_refs),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        e
    }

    /// REQSET operand bytes: priority @1, target actor @2, tag @6.
    fn reqset_operands(priority: u8, actor: u32, tag: u8) -> Vec<u8> {
        let mut o = vec![priority];
        o.extend_from_slice(&actor.to_le_bytes());
        o.push(tag);
        o
    }

    /// REQWAIT operand bytes: priority @1, target actor @2.
    fn reqwait_operands(priority: u8, actor: u32) -> Vec<u8> {
        let mut o = vec![priority];
        o.extend_from_slice(&actor.to_le_bytes());
        o
    }

    /// Master: `OP_REQSET` the NPC's tag 1 at priority 0, then END; the NPC
    /// block hides itself and ends (research/XiEvents/OpCodes/0x0027.md).
    /// The child's first frame runs on the next step; its cue bubbles up with
    /// the NPC as event entity.
    #[test]
    fn reqset_spawns_child_on_target_block_at_tag_index() {
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(0, NPC_SERVER_ID, 1));
        master.push(OP_END);
        let mut npc = vec![OP_EVENTHIDE, 1];
        npc.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        npc.push(OP_END);

        let mut e = scene_vm(master, npc);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master ends but its request stack still holds work"
        );
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(
            e.take_cues(),
            [EventCue::ActorHide {
                target: ActorLookup(NPC_SERVER_ID),
                hide: true,
            }]
        );
    }

    /// Master: `OP_REQSET` the NPC's tag 1 at priority 3, then `OP_REQWAIT`
    /// priority 3; the NPC parks on a ten-second timed wait. A numerically
    /// higher priority on the stack does not hold a lower REQWAIT byte
    /// (research/XiEvents/OpCodes/0x002A.md).
    #[test]
    fn reqwait_holds_until_target_stack_drains_at_or_below_priority() {
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(3, NPC_SERVER_ID, 1));
        master.push(OP_REQWAIT);
        master.extend_from_slice(&reqwait_operands(3, NPC_SERVER_ID));
        master.push(OP_END);
        const TEN_SECONDS: u32 = 10 * WAIT_UNITS_PER_SEC as u32;
        let npc = vec![OP_WAIT, REF0[0], REF0[1]];

        let mut e = scene_vm_refs(master, npc.clone(), vec![TEN_SECONDS]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the REQWAIT parks on its own push"
        );
        assert_eq!(e.step(), StepResult::Waiting);
        e.tick(5.0);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "halfway through the child's wait"
        );
        e.tick(5.1);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the drained stack releases the REQWAIT"
        );

        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(110, NPC_SERVER_ID, 1));
        master.push(OP_REQWAIT);
        master.extend_from_slice(&reqwait_operands(3, NPC_SERVER_ID));
        master.push(OP_END);
        let mut e = scene_vm_refs(master, npc, vec![TEN_SECONDS]);
        assert_eq!(e.step(), StepResult::Waiting, "the stack still holds work");
        assert_eq!(e.step(), StepResult::Waiting, "the child arms its wait");
        e.tick(10.5);
        assert_eq!(e.step(), StepResult::Done);
    }

    /// Master: `OP_REQSET_PRIORITY` the NPC's tag 1, then END; the NPC parks on
    /// a one-second timed wait (research/XiEvents/OpCodes/0x0029.md).
    #[test]
    fn reqew_pushes_then_waits_for_that_tag() {
        let mut master = vec![OP_REQSET_PRIORITY];
        master.extend_from_slice(&reqset_operands(5, NPC_SERVER_ID, 1));
        master.push(OP_END);
        const ONE_SECOND: u32 = WAIT_UNITS_PER_SEC as u32;
        let npc = vec![OP_WAIT, REF0[0], REF0[1]];

        let mut e = scene_vm_refs(master, npc, vec![ONE_SECOND]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "REQEW holds while its tag still sits on the stack"
        );
        assert_eq!(e.step(), StepResult::Waiting);
        e.tick(1.5);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the tag leaves the stack when its request ends"
        );
    }

    /// One NPC block, four tags: tag 2 is a one-second wait, tag 3 hides and
    /// ends. The master REQSETs both; the lower number runs first, and the
    /// other starts only when it drains (research/XiEvents/OpCodes/0x0027.md).
    /// NPC block: [0..3) tag 2: one-second wait; [3..10) tag 3: hide + END.
    #[test]
    fn lower_priority_number_preempts_and_the_other_resumes() {
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(9, NPC_SERVER_ID, 2));
        master.push(OP_REQSET);
        master.extend_from_slice(&reqset_operands(1, NPC_SERVER_ID, 3));
        master.push(OP_END);
        let mut npc = vec![OP_WAIT, REF0[0], REF0[1], OP_EVENTHIDE, 1];
        npc.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        npc.push(OP_END);

        const ONE_SECOND: u32 = WAIT_UNITS_PER_SEC as u32;
        let mut dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![block(master.clone(), vec![])],
        };
        dat.blocks.push(EventBlock {
            actor: NPC_SERVER_ID,
            event_ids: vec![7; 4],
            event_offsets: vec![0, 0, 0, 3],
            references: vec![ONE_SECOND],
            event_data: npc,
        });

        let mut e = vm(master, vec![]);
        e.attach_scene(
            std::sync::Arc::new(dat),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        assert_eq!(e.step(), StepResult::Waiting, "both requests are queued");
        assert_eq!(
            e.take_cues().len(),
            0,
            "the higher-numbered request has not run yet"
        );
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "tag 1 ends; tag 0 still queued"
        );
        assert_eq!(
            e.take_cues(),
            [EventCue::ActorHide {
                target: ActorLookup(NPC_SERVER_ID),
                hide: true,
            }],
            "the lower-numbered request ran first, from its saved pointer"
        );
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "tag 0 starts and arms its one-second wait"
        );
        e.tick(1.5);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the drained stack ends the event"
        );
    }

    /// An NPC block with 17 placeholder entries, all starting at offset 0 of a
    /// one-second wait; the master REQSETs tags 1..16 (sixteen pushes fill the
    /// stack) and then tag 0, which yields on the full stack — the tag-0
    /// no-op only applies while some slot is still zeroed
    /// (research/XiEvents/OpCodes/0x0028.md).
    #[test]
    fn stack_full_makes_reqset_yield() {
        let mut dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![block(vec![], vec![])],
        };
        let npc_block = EventBlock {
            actor: NPC_SERVER_ID,
            event_ids: vec![7; 17],
            event_offsets: vec![0; 17],
            references: vec![WAIT_UNITS_PER_SEC as u32],
            event_data: vec![OP_WAIT, REF0[0], REF0[1]],
        };
        dat.blocks.push(npc_block);

        let mut master = Vec::new();
        for tag in 1u8..17 {
            master.push(OP_REQSET_CHECKED);
            master.extend_from_slice(&reqset_operands(0, NPC_SERVER_ID, tag));
        }
        master.push(OP_REQSET);
        master.extend_from_slice(&reqset_operands(0, NPC_SERVER_ID, 0));
        master.push(OP_END);

        let mut e = vm(master, vec![]);
        e.attach_scene(
            std::sync::Arc::new(dat),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the tag-0 push finds a full stack and yields"
        );
    }

    /// Retail's ReqSet walks all 16 ReqStack slots and returns 0 on any
    /// matching TagNum, including the zeroed TagNum of unused slots, so a
    /// REQSET of tag 0 spawns no child while the target's stack is not full.
    /// The master REQSETs tag 0 on an idle NPC and runs straight through to
    /// its END (research/XiEvents/OpCodes/0x0027.md).
    #[test]
    fn reqset_tag_zero_is_always_a_noop_on_a_non_full_stack() {
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(0, NPC_SERVER_ID, 0));
        master.push(OP_END);
        let mut npc = vec![OP_EVENTHIDE, 1];
        npc.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        npc.push(OP_END);

        let mut e = scene_vm(master, npc);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the tag-0 REQSET is a no-op and the master's END finishes the event"
        );
        assert!(
            !e.scene_waiting(),
            "no child was queued, so no stack holds work"
        );
        assert!(
            e.take_cues().is_empty(),
            "no child ran, so no cue may surface"
        );
    }

    /// The NPC block's second entry carries the placeholder event id; REQSET
    /// by tag index still reaches it, because ReqSet indexes TagOffset
    /// directly (research/XiEvents/OpCodes/0x0027.md).
    #[test]
    fn placeholder_tag_entries_are_reqset_entry_points() {
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(0, NPC_SERVER_ID, 1));
        master.push(OP_END);
        let mut npc_data = vec![OP_EVENTHIDE, 1];
        npc_data.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        npc_data.push(OP_END);
        let npc_block = EventBlock {
            actor: NPC_SERVER_ID,
            event_ids: vec![7, ffxi_dat::event_dat::EVENT_ID_PLACEHOLDER],
            event_offsets: vec![0, 0],
            references: vec![],
            event_data: npc_data,
        };
        let mut dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![block(master.clone(), vec![])],
        };
        dat.blocks.push(npc_block);

        let mut e = vm(master, vec![]);
        e.attach_scene(
            std::sync::Arc::new(dat),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master ends; the child is queued"
        );
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(
            e.take_cues(),
            [EventCue::ActorHide {
                target: ActorLookup(NPC_SERVER_ID),
                hide: true,
            }]
        );
    }

    /// Master: `OP_REQSET` the NPC's tag 0, then END. The NPC sets its speed,
    /// walks to a goal (case 0), and holds on the arrival test (case 1)
    /// (research/XiEvents/OpCodes/0x001F.md). NPC block: [0..3) speed =
    /// refs[0] * 0.1; [3..11) MOVE case 0 goal: x @2, z @4 and y @6 as work
    /// operands, all through the References table because a plain bytecode
    /// value is a work-store index; [11..13) MOVE case 1; [13] END. A
    /// host-armed move hold, set before the child's first frame, parks case 1
    /// until it expires.
    #[test]
    fn npc_move_emits_actor_move_and_case1_holds_on_host_hold() {
        let npc = vec![
            OP_SPEED_TEST,
            REF0[0],
            REF0[1],
            OP_MOVE_TEST,
            0,
            REF2[0],
            REF2[1],
            REF3[0],
            REF3[1],
            REF1[0],
            REF1[1],
            OP_MOVE_TEST,
            1,
            OP_END,
        ];

        /// References[0]: the raw speed operand; * EVENT_SPEED_SCALE it is 1.0 yalm/s.
        const MOVE_SPEED_REF: u32 = 10;
        /// References[1]: the y goal -5 (a bytecode literal cannot carry it).
        const NEG_FIVE_REF: u32 = (-5_i32) as u32;
        /// References[2]: the x goal.
        const GOAL_X_REF: u32 = 20;
        /// References[3]: the z goal.
        const GOAL_Z_REF: u32 = 40;

        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(0, NPC_SERVER_ID, 1));
        master.push(OP_END);
        let mut dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![block(master.clone(), vec![])],
        };
        dat.blocks.push(EventBlock {
            actor: NPC_SERVER_ID,
            event_ids: vec![7, 7],
            event_offsets: vec![0, 0],
            references: vec![MOVE_SPEED_REF, NEG_FIVE_REF, GOAL_X_REF, GOAL_Z_REF],
            event_data: npc.clone(),
        });

        let mut e = vm(master, vec![]);
        e.attach_scene(
            std::sync::Arc::new(dat),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master ends; the child is queued"
        );
        assert_eq!(e.step(), StepResult::Done);
        let cues = e.take_cues();
        assert_eq!(
            cues,
            [EventCue::ActorMove {
                actor: ActorLookup(NPC_SERVER_ID),
                goal: crate::vm::scene::EventPosition {
                    x: 20,
                    y: -5,
                    z: 40,
                    heading: 0,
                },
                speed: MOVE_SPEED_REF as i32,
                max_time: None,
            }]
        );

        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(0, NPC_SERVER_ID, 1));
        master.push(OP_END);
        let mut dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![block(master.clone(), vec![])],
        };
        dat.blocks.push(EventBlock {
            actor: NPC_SERVER_ID,
            event_ids: vec![7, 7],
            event_offsets: vec![0, 0],
            references: vec![MOVE_SPEED_REF, NEG_FIVE_REF, GOAL_X_REF, GOAL_Z_REF],
            event_data: npc,
        });

        let mut e = vm(master, vec![]);
        e.attach_scene(
            std::sync::Arc::new(dat),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master ends; the child is queued"
        );
        e.hold_move(ActorLookup(NPC_SERVER_ID), WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "case 1 parks on the armed hold"
        );
        assert_eq!(e.take_cues().len(), 1, "the move cue went out with case 0");
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the hold still has frames left"
        );
        e.tick(1.5);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "an expired move hold falls through"
        );
    }

    /// Event 503's guard tag[2] shape: the master REQSETs a one-line sub-script
    /// (speakerless PRINT_MSG + MESWAIT + END) onto an NPC and then REQWAITs on
    /// that actor. The child's line must surface to the host, the dismissal must
    /// reach the child, and its END must drain the stack so the REQWAIT passes.
    /// The child's first frame opens its line with the message id from the NPC
    /// block's references table; the re-step parks while the frame is up, and
    /// the master's REQWAIT still holds because the child request has not
    /// drained.
    #[test]
    fn child_dialog_frame_surfaces_and_dismissal_releases_the_reqwait() {
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(110, NPC_SERVER_ID, 1));
        master.push(OP_REQWAIT);
        master.extend_from_slice(&reqwait_operands(110, NPC_SERVER_ID));
        master.push(OP_END);
        let npc = vec![OP_MESSAGE_UNNAMED, REF0[0], REF0[1], OP_MESWAIT, OP_END];

        let mut e = scene_vm_refs(master, npc, vec![MSG_ID]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master ends; the child is queued behind its REQWAIT"
        );
        let StepResult::AwaitMessage(m) = e.step() else {
            panic!("the child's dialog frame did not surface");
        };
        assert_eq!(m.message_id, MSG_ID);
        assert_eq!(
            m.speaker_index, None,
            "0x48 prints through the nameless path"
        );
        assert_eq!(e.step(), StepResult::Waiting);
        e.dismiss_message();
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the dismissal reaches the child; its END drains the stack and releases the REQWAIT"
        );
    }

    /// Two children that both park on a dialog in one pass: retail's single
    /// global open flag holds one frame at a time, so only the first surfaces;
    /// the second is dropped rather than stalling its own stack behind it.
    #[test]
    fn second_child_parking_on_a_dialog_is_dropped_while_the_first_frame_stays_up() {
        const NPC2_SERVER_ID: u32 = 0x0100_02C6;
        let line = vec![OP_MESSAGE_UNNAMED, REF0[0], REF0[1], OP_MESWAIT, OP_END];

        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(1, NPC_SERVER_ID, 1));
        master.push(OP_REQSET);
        master.extend_from_slice(&reqset_operands(1, NPC2_SERVER_ID, 1));
        master.push(OP_END);

        let mut dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![block(master.clone(), vec![])],
        };
        for actor in [NPC_SERVER_ID, NPC2_SERVER_ID] {
            dat.blocks.push(EventBlock {
                actor,
                event_ids: vec![7, 7],
                event_offsets: vec![0, 0],
                references: vec![MSG_ID],
                event_data: line.clone(),
            });
        }

        let mut e = vm(master, vec![]);
        e.attach_scene(
            std::sync::Arc::new(dat),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        assert_eq!(e.step(), StepResult::Waiting, "both requests are queued");
        let StepResult::AwaitMessage(_) = e.step() else {
            panic!("the first child's dialog frame did not surface");
        };
        e.dismiss_message();
        assert_eq!(
            e.step(),
            StepResult::Done,
            "only the surfaced child remains; its END drains the last stack"
        );
    }

    /// A child that stops on an opcode this VM does not run must actually leave
    /// its actor's stack: a zombie request would keep the master's REQWAIT
    /// parked forever. Event 503's party walk-in children hit exactly this at
    /// their CodeMOVE2 pair, which stalled the event between E3 and E4. The
    /// `OP_LOADROOM` with an undocumented sub stops the VM (sub_size has no
    /// width for it), so the child cannot drain on its own; the first frame
    /// stops there and is dropped, and the REQWAIT sees an empty stack so the
    /// event can finish.
    #[test]
    fn child_stopped_on_unrunnable_opcode_is_dropped_and_releases_the_reqwait() {
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(3, NPC_SERVER_ID, 1));
        master.push(OP_REQWAIT);
        master.extend_from_slice(&reqwait_operands(3, NPC_SERVER_ID));
        master.push(OP_END);
        let npc = vec![OP_LOADROOM, 0xFF, 0, 0, OP_END];

        let mut e = scene_vm_refs(master, npc, vec![]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master ends; the child is queued behind its REQWAIT"
        );
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the unrunnable child is dropped; its REQWAIT releases"
        );
    }

    /// CodeMOVE2 is retail's uncalibrated twin of MOVE: case 0 emits the walk
    /// cue, case 1 holds on the host-armed move hold and advances exactly two
    /// bytes so the next opcode still runs. Event 503's party children open
    /// their tag[2] walk-in with this pair. NPC block: [0..3) speed = refs[0] * EVENT_SPEED_SCALE;
    /// [3..11) CodeMOVE2 case 0 goal: x @2, z @4 and y @6 through the
    /// References table; [11..13) CodeMOVE2 case 1; then an EVENTHIDE cue that
    /// only runs if case 1 advanced two bytes instead of the table's widest
    /// eight. A host-armed move hold, set before the child's first frame,
    /// parks case 1 until it expires.
    #[test]
    fn code_move2_walks_like_move_on_an_npc_child() {
        const CODE_MOVE2: u8 = 0x5A;
        let mut npc = vec![OP_SPEED_TEST, REF0[0], REF0[1]];
        npc.extend_from_slice(&[CODE_MOVE2, 0]);
        npc.extend_from_slice(&REF2);
        npc.extend_from_slice(&REF3);
        npc.extend_from_slice(&REF1);
        npc.extend_from_slice(&[CODE_MOVE2, 1]);
        npc.push(OP_EVENTHIDE);
        npc.push(1);
        npc.extend_from_slice(&NPC_SERVER_ID.to_le_bytes());
        npc.push(OP_END);

        /// References[0]: the raw speed operand; * EVENT_SPEED_SCALE it is 1.0 yalm/s.
        const SPEED_REF: u32 = 10;
        /// References[1]: the y goal -5 (a bytecode literal cannot carry it).
        const NEG_FIVE_REF: u32 = (-5_i32) as u32;
        /// References[2]: the x goal.
        const GOAL_X_REF: u32 = 20;
        /// References[3]: the z goal.
        const GOAL_Z_REF: u32 = 40;

        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(0, NPC_SERVER_ID, 1));
        master.push(OP_END);
        let mut dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![block(master.clone(), vec![])],
        };
        dat.blocks.push(EventBlock {
            actor: NPC_SERVER_ID,
            event_ids: vec![7, 7],
            event_offsets: vec![0, 0],
            references: vec![SPEED_REF, NEG_FIVE_REF, GOAL_X_REF, GOAL_Z_REF],
            event_data: npc.clone(),
        });

        let mut e = vm(master.clone(), vec![]);
        e.attach_scene(
            std::sync::Arc::new(dat),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master ends; the child is queued"
        );
        assert_eq!(e.step(), StepResult::Done);
        let cues = e.take_cues();
        assert_eq!(
            cues,
            [
                EventCue::ActorMove {
                    actor: ActorLookup(NPC_SERVER_ID),
                    goal: crate::vm::scene::EventPosition {
                        x: 20,
                        y: -5,
                        z: 40,
                        heading: 0,
                    },
                    speed: SPEED_REF as i32,
                    max_time: None,
                },
                EventCue::ActorHide {
                    target: ActorLookup(NPC_SERVER_ID),
                    hide: true,
                },
            ]
        );

        let mut dat = ffxi_dat::event_dat::EventDat {
            blocks: vec![block(master.clone(), vec![])],
        };
        dat.blocks.push(EventBlock {
            actor: NPC_SERVER_ID,
            event_ids: vec![7, 7],
            event_offsets: vec![0, 0],
            references: vec![SPEED_REF, NEG_FIVE_REF, GOAL_X_REF, GOAL_Z_REF],
            event_data: npc,
        });

        let mut e = vm(master.clone(), vec![]);
        e.attach_scene(
            std::sync::Arc::new(dat),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            crate::vm::scene::EventPosition::default(),
        );
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master ends; the child is queued"
        );
        e.hold_move(ActorLookup(NPC_SERVER_ID), WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "case 1 parks on the armed hold"
        );
        assert_eq!(e.take_cues().len(), 1, "the move cue went out with case 0");
        e.tick(1.5);
        assert_eq!(
            e.step(),
            StepResult::Done,
            "an expired move hold falls through"
        );
    }

    /// Trigger-packet parameters ride along on every message opcode.
    #[test]
    fn params_flow_through_every_message_opcode() {
        let params = vec![3, 4];
        for (op, operands) in [
            (OP_MESSAGE, REF0.to_vec()),
            (OP_MESSAGE_UNNAMED, REF0.to_vec()),
            (OP_MESSAGE_ACTOR, {
                let mut o = NPC_SERVER_ID.to_le_bytes().to_vec();
                o.extend_from_slice(&REF0);
                o
            }),
        ] {
            let data = message_program(op, &operands);
            let mut e = EventVm::start(&block(data, vec![MSG_ID]), 7, 5, params.clone()).unwrap();
            let StepResult::AwaitMessage(m) = e.step() else {
                panic!("op 0x{op:02X} produced no message");
            };
            assert_eq!(m.params, params, "op 0x{op:02X} dropped event params");
        }
    }

    /// 0xB4 case 0: the inline 16-byte literal at +4 fills the work string the
    /// +2 operand selects (research/XiEvents/OpCodes/0x00B4.md).
    #[test]
    fn window_case_zero_copies_the_inline_literal_into_the_work_string() {
        let literal: [u8; 16] = *b"Sajj'aka\0\0\0\0\0\0\0\0";
        let mut data = vec![OP_WINDOW, 0x00];
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(&literal);
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 20);
        assert_eq!(e.work_local_str[3], literal);
    }

    /// 0xB4 case 1: the +4 work operand is the PENDINGSTR index, routed through
    /// the same getworkofs as every other operand — here a References-flagged
    /// value naming refs[7] (research/XiEvents/OpCodes/0x00B4.md).
    #[test]
    fn window_case_one_reads_the_pending_str_entry_the_work_operand_selects() {
        let mut data = vec![OP_WINDOW, 0x01];
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(&REF7);
        data.push(OP_END);
        let mut pending = [[0u8; 16]; 4];
        pending[2] = *b"pending-two\0\0\0\0\0";
        let mut e = vm(data, vec![0, 0, 0, 0, 0, 0, 0, 2]);
        e.apply_pending_str(&pending);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.work_local_str[3], pending[2]);
    }

    /// The `OP_WINDOW` case 1 with a literal index past the four-entry table:
    /// retail lands on entry 0, not the clamped last entry.
    #[test]
    fn window_case_one_out_of_range_index_reads_slot_zero() {
        let mut data = vec![OP_WINDOW, 0x01];
        data.extend_from_slice(&3u16.to_le_bytes());
        data.extend_from_slice(&9u16.to_le_bytes());
        data.push(OP_END);
        let mut pending = [[0u8; 16]; 4];
        pending[0] = *b"zero\0\0\0\0\0\0\0\0\0\0\0\0";
        pending[3] = *b"three\0\0\0\0\0\0\0\0\0\0\0";
        let mut e = vm(data, vec![]);
        e.apply_pending_str(&pending);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(
            e.work_local_str[3], pending[0],
            "out of range lands on entry 0"
        );
    }

    /// 0xB5 case 0: the event entity's display name becomes the work string the
    /// +2 operand selects; the canonical rename is a 0xB4 case 0 right before
    /// it (research/XiEvents/OpCodes/0x00B5.md).
    #[test]
    fn nameset_case_zero_emits_the_event_entity_name_cue() {
        let literal: [u8; 16] = *b"Sajj'aka\0\0\0\0\0\0\0\0";
        let mut data = vec![OP_WINDOW, 0x00];
        data.extend_from_slice(&5u16.to_le_bytes());
        data.extend_from_slice(&literal);
        data.extend_from_slice(&[OP_NAMESET, 0x00]);
        data.extend_from_slice(&5u16.to_le_bytes());
        data.push(OP_END);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 24);
        assert_eq!(
            e.take_cues(),
            vec![EventCue::EntityName {
                actor: ActorLookup::EVENT_ENTITY,
                name: literal,
            }]
        );
    }

    /// 0xDA: the single retail site (zone 112 event 68) is a 6-byte header and
    /// six 28-byte records, each naming (event entity, event entity, "senN").
    /// The VM emits one ActorMotion cue per record and advances past all of
    /// them, landing on the next real instruction.
    #[test]
    fn da_batch_motion_loader_emits_one_cue_per_record_and_advances_past_them() {
        let actor = 0x7FFFFFF8u32.to_le_bytes();
        let mut data = vec![0xDA, 0x0C, 0x00, 0x02, 0x0C, 0x00];
        for i in 0..6u8 {
            let mut rec = [0u8; 28];
            rec[8..12].copy_from_slice(&actor);
            rec[12..16].copy_from_slice(&actor);
            rec[16..20].copy_from_slice(&[b's', b'e', b'n', b'0' + i]);
            data.extend_from_slice(&rec);
        }
        // The scan must stop at the first record whose key is not a printable
        // 4cc: this one's key field is 0x80 0x27 0x10 0xF0.
        let mut term = [0u8; 28];
        term[16..20].copy_from_slice(&[0x80, 0x27, 0x10, 0xF0]);
        data.extend_from_slice(&term);
        let mut e = vm(data, vec![]);
        assert_eq!(e.step(), StepResult::Done);
        let cues = e.take_cues();
        assert_eq!(cues.len(), 6, "six records, six cues");
        for (i, cue) in cues.iter().enumerate() {
            match cue {
                EventCue::ActorMotion {
                    actor1,
                    actor2,
                    key,
                } => {
                    assert_eq!(*actor1, ActorLookup::EVENT_ENTITY);
                    assert_eq!(*actor2, ActorLookup::EVENT_ENTITY);
                    assert_eq!(*key, [b's', b'e', b'n', b'0' + i as u8]);
                }
                other => panic!("expected ActorMotion, got {other:?}"),
            }
        }
    }

    /// setworkstrofs refuses both stores: a References-flagged operand is
    /// read-only, and the write bound is slot 64 even though the int view runs
    /// to 80 (research/XiEvents/Event VM Functions.md).
    #[test]
    fn window_case_zero_refuses_ref_flagged_and_over_bound_destinations() {
        let literal: [u8; 16] = *b"Sajj'aka\0\0\0\0\0\0\0\0";
        for dest in [0x8003u16, 64] {
            let mut data = vec![OP_WINDOW, 0x00];
            data.extend_from_slice(&dest.to_le_bytes());
            data.extend_from_slice(&literal);
            data.push(OP_END);
            let mut e = vm(data, vec![0, 0, 0, 0]);
            assert_eq!(e.step(), StepResult::Done);
            assert_eq!(
                e.work_local_str, [[0u8; 16]; WORK_LOCAL_LEN],
                "dest 0x{dest:04X} must not store"
            );
        }
    }

    /// Copy event params 0..4 into work_local 0..4 (0x03 GET_STORE, 5-byte
    /// width): the 0x37/0x39 tests read their operands from work slots the
    /// way the authored programs do.
    fn store_params_to_work_local(data: &mut Vec<u8>) {
        for i in 0..4u16 {
            data.push(OP_GET_STORE);
            data.extend_from_slice(&i.to_le_bytes());
            data.extend_from_slice(&(4098u16 + i).to_le_bytes());
        }
    }

    fn scene_player_vm(
        data: Vec<u8>,
        params: Vec<i32>,
        start: crate::vm::scene::EventPosition,
    ) -> EventVm {
        let block = block(data, vec![]);
        let mut e = EventVm::start(&block, 7, 5, params).unwrap();
        e.attach_scene(
            std::sync::Arc::new(ffxi_dat::event_dat::EventDat {
                blocks: vec![block],
            }),
            ffxi_dat::event_dat::ZONE_PLAYER_ACTOR,
            start,
        );
        e
    }

    /// 0x37 on the player: the authored position and facing become the
    /// tracked player position at once, published as one PlayerPosition scene
    /// action (the renderer's snap and the c2s POS ride on it); the walks
    /// that follow start from here.
    #[test]
    fn set_event_pos_on_the_player_snaps_the_tracked_position() {
        let authored = crate::vm::scene::EventPosition {
            x: -56_030,
            y: 8_000,
            z: 109_070,
            heading: 1590,
        };
        let mut data = Vec::new();
        store_params_to_work_local(&mut data);
        data.push(0x37);
        for i in 0..4u16 {
            data.extend_from_slice(&i.to_le_bytes());
        }
        data.push(OP_END);
        let mut e = scene_player_vm(
            data,
            vec![authored.x, authored.z, authored.y, authored.heading],
            crate::vm::scene::EventPosition::default(),
        );
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(
            e.take_scene_actions(),
            [crate::vm::scene::SceneAction::PlayerPosition(authored)]
        );
        assert_eq!(e.controlled_position(), Some(authored));
    }

    /// 0x39 on the player: the authored facing becomes the tracked heading and
    /// the position is republished so the rendered body turns onto it.
    #[test]
    fn set_facing_on_the_player_republishes_the_heading() {
        let mut data = Vec::new();
        store_params_to_work_local(&mut data);
        data.push(0x39);
        data.extend_from_slice(&3u16.to_le_bytes());
        data.push(OP_END);
        let start = crate::vm::scene::EventPosition {
            x: -56_030,
            y: 8_000,
            z: 109_070,
            heading: 0,
        };
        let mut e = scene_player_vm(
            data,
            vec![start.x, start.z, start.y, EVENT_MOTION_BAND_4],
            start,
        );
        assert_eq!(e.step(), StepResult::Done);
        let expected = crate::vm::scene::EventPosition {
            x: -56_030,
            y: 8_000,
            z: 109_070,
            heading: EVENT_MOTION_BAND_4,
        };
        assert_eq!(
            e.take_scene_actions(),
            [crate::vm::scene::SceneAction::PlayerPosition(expected)]
        );
        assert_eq!(e.controlled_position(), Some(expected));
    }
}
