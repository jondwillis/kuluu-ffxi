pub mod scene;

use std::sync::{Arc, Mutex};

use ffxi_dat::event_dat::EventBlock;

use crate::cue::{
    dat_id_helper, event_motion_dat_id, tpc_motion_packages, ActorLookup, EventCue,
    ExtSchedulerMotion, FourCc, MUSIC_VOLUME_MAX, NO_ACTION_KEY, SCHEDULER_DAT_ID_BASE,
    STATUS_EVENT_CHOCOBO, STATUS_EVENT_IDLE, STATUS_EVENT_MOUNT,
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
const OP_DEFCAMERA: u8 = 0x46;
const OP_EVENTHIDE: u8 = 0x4E;
const OP_CLOSE_MAP: u8 = 0x8A;
const OP_MAP_MARKER: u8 = 0x8B;
const OP_MAP_TUTORIAL: u8 = 0xC8;
const OP_HIDE_HUD: u8 = 0x67;
const OP_SHOW_HUD: u8 = 0x68;
const OP_STOP_CLOCK: u8 = 0x77;
const OP_RESTORE_CLOCK: u8 = 0x78;
const OP_MUSICVOLUME: u8 = 0x5D;
const OP_WAITSCHEDULOR: u8 = 0x53;
const OP_WAITMAPSCHEDULOR: u8 = 0x54;
const OP_WAITLOADSCHEDULER: u8 = 0x55;
const OP_CHOCOBO: u8 = 0x7E;
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

// Advances the actor-driven wait opcodes take when their entity does not
// resolve — retail's own `!GetActorIndex` / `!entity` early exit, which is the
// only path this actor-less VM can be on (research/XiEvents/OpCodes/0x0080.md,
// 0x0076.md, 0x0099.md, 0x006E.md, 0x006C.md, 0x0063.md).
const LOADWAIT_SIZE: usize = 5;
const EMOT_SIZE: usize = 7;
const TRANSPAR_SIZE: usize = 9;
const PLAYANIM_SIZE: usize = 3;
/// 0x0034.md (and 0x0035.md, the same handler without the zone close) spreads
/// its zone load over three `EventIdle` ticks driven by two file-scope counters,
/// advancing only on the last; the net effect of the sequence is +3, and this VM
/// has no frame clock to spend the first two on.
const MAPLOAD_SIZE: usize = 3;
/// 0x0058.md is `ExecPointer++; RetFlag = 1`; 0x009A.md yields only while the
/// music server is mid-read, which nothing here triggers.
const YIELD_SIZE: usize = 1;

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
const LOADEXTSCHEDULER_FILE_OFS: usize = 1; // 0x005B / 0x0066
const LOADEXTSCHEDULER_ACTOR1_OFS: usize = 3;
const LOADEXTSCHEDULER_ACTOR2_OFS: usize = 7;
const LOADEXTSCHEDULER_KEY_OFS: usize = 11;
// The WAIT* family's host-armed hold matches on (actor1, key) only; retail's
// IsMovingAction also takes actor2 (0x53/0x54 @5, 0x55 @7), but the partner does
// not change which armed hold a wait parks on.
const WAITSCHEDULOR_ACTOR1_OFS: usize = 1; // 0x0053 / 0x0054
const WAITSCHEDULOR_KEY_OFS: usize = 9;
const WAITLOADSCHEDULER_ACTOR1_OFS: usize = 3; // 0x0055
const WAITLOADSCHEDULER_KEY_OFS: usize = 11;
const MAPSCHEDULOR_KEY_OFS: usize = 9; // 0x002D, same layout as the WAIT family
const MAPSCHEDULOR_ACTOR2_OFS: usize = 5; // 0x002D partner slot of that layout
const DEFCAMERA_CASE_OFS: usize = 1; // 0x0046
const DEFCAMERA_CASE_UNLOCK: u8 = 0;
const DEFCAMERA_CASE_LOCK: u8 = 1;
const EVENTHIDE_FLAG_OFS: usize = 1; // 0x004E
const EVENTHIDE_FLAG_MASK: u8 = 1;
const EVENTHIDE_TARGET_OFS: usize = 2;
const MUSICVOLUME_LEVEL_OFS: usize = 1; // 0x005D
const MUSICVOLUME_FADE_OFS: usize = 3;
/// 0x77's hour operand (research/XiEvents/OpCodes/0x0077.md); its weather
/// half is server-driven here, so it has no cue.
const STOP_CLOCK_HOUR_OFS: usize = 1;
/// 0x77's "no time change" sentinel for the hour operand.
const STOP_CLOCK_NO_HOUR: i32 = 255;
const MAP_OPEN_ID_OFS: usize = 1; // 0x00C8
const MAP_OPEN_TUTORIAL_OFS: usize = 5; // 0x00C8, LOBYTE is the bool
const MAP_MARKER_ID_OFS: usize = 1; // 0x008B
const MAP_MARKER_X_OFS: usize = 5; // 0x008B
const MAP_MARKER_Y_OFS: usize = 7; // 0x008B
const MAP_MARKER_NAME_OFS: usize = 9; // 0x008B, 16 bytes
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

const WORK_LOCAL_LEN: usize = 80;
// `XiEvent::setworkstrofs` refuses string writes at slot 64 and up: a 16-byte
// store must stay inside the WorkLocal table (research/XiEvents/Event VM
// Functions.md; the int view bounds the same table at 80 slots).
const WORK_STR_WRITE_LEN: usize = 64;
const WORK_ZONE_LEN: usize = 96;
const WORK_ZONE_BASE: u32 = 4096;
// Selbina's Lucia event 221 reads num[0] as Work_Zone[2]; Southern San d'Oria
// event 599 reads num[1..2] as Work_Zone[3..4]. Both are retail event DATs.
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
    /// The s2c 0x005D PENDINGSTR table (PTR_EventStrings): four 16-byte strings
    /// the server pushes before the event, read by 0xB4 case 1.
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
    /// at start (plain conversations stay cancellable); 0x42 disarms it in the
    /// prologue of cutscenes that lock you in, 0x2E re-arms it.
    cancel_armed: bool,
    /// Choreography cues emitted since the host last drained them — see
    /// [`Self::take_cues`].
    cues: Vec<EventCue>,
    finished: bool,
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
    /// The retail entity Type byte (ent+0xEE) of the actors this VM's
    /// 0x5B/0x66 opcodes name, keyed by the actor's server id and target
    /// index: the gate both motion resource readers apply before loading.
    /// An absent entry is Type 0 — retail's value when the entity has no
    /// back-ptr.
    actor_types: std::collections::HashMap<u32, u8>,
    /// Actions this VM's own 0x45/0x5B opcodes started within the current step,
    /// before the host has drained the cues and armed their holds. They bridge a
    /// loader to its WAIT* when both run in one pass; [`Self::take_cues`] and the
    /// start of each later [`step`](Self::step) clear them, so an action whose DAT
    /// the host cannot read falls through instead of holding forever.
    pending_action_starts: Vec<(ActorLookup, FourCc)>,
    /// Motion holds the renderer's finish report releases instead of a timer:
    /// every routine the host plays and reports (0x2C, 0x45 non-fade,
    /// 0x5B/0x66, 0x2D) parks here while it runs. See
    /// [`Self::hold_action_pending`].
    pending_action_holds: Vec<(ActorLookup, FourCc)>,
    /// Set while execution is parked on a WAIT* opcode whose hold still has
    /// frames left, so [`Self::is_waiting`] keeps the host ticking it down.
    parked_on_action_hold: bool,
    /// The same for a non-player MOVE case 1 parked on its move hold.
    parked_on_move_hold: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Wait {
    remaining_units: f32,
    advance: usize,
}

/// A running action the host told the VM about, so the WAIT* family can hold
/// the way retail's IsMovingAction does. Armed by the host from the
/// DAT-authored routine length when it publishes a motion cue the renderer
/// plays without a finish report (the 0x45 fades); the VM never invents one,
/// so an un-armed wait falls through.
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
            ran_past_end: false,
            wait: None,
            pending_ack: None,
            req_wait: None,
            action_holds: Vec::new(),
            move_holds: Vec::new(),
            actor_types: std::collections::HashMap::new(),
            pending_action_starts: Vec::new(),
            pending_action_holds: Vec::new(),
            parked_on_action_hold: false,
            parked_on_move_hold: false,
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

    /// Drain the [`EventCue`]s the staging opcodes emitted, in execution order.
    /// They accumulate across [`step`](Self::step) calls (one step can emit
    /// several), so the host drains after each step rather than reading a
    /// per-step return value.
    pub fn take_cues(&mut self) -> Vec<EventCue> {
        let cues = std::mem::take(&mut self.cues);
        // The host arms holds from these cues now; the same-batch bridge is
        // spent, and anything it covered that no hold confirms must fall through.
        self.pending_action_starts.clear();
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
    /// as [`Self::tick`]). Replaces an existing hold for the same pair. The
    /// event-entity selector resolves against the running scene's actor, the
    /// way [`EventCue::resolve_event_actor`] does, so the host passes the
    /// unresolved lookup it got from the cue.
    pub fn hold_action(&mut self, actor: ActorLookup, key: FourCc, units: f32) {
        let actor = self.resolve_hold_actor(actor);
        self.action_holds
            .retain(|h| !(h.actor == actor && h.key == key));
        // Last-arm-wins both ways: a timed hold supersedes the pending motion
        // hold for the same pair.
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
    /// the VM no longer waits on (stopped, superseded, event ended) is safe.
    pub fn release_action_hold(&mut self, actor: ActorLookup, key: FourCc) {
        let actor = self.resolve_hold_actor(actor);
        self.pending_action_holds
            .retain(|(a, k)| !(a == &actor && k == &key));
    }

    /// Replace the entity Type table the 0x5B/0x66 gate reads (see
    /// [`Self::actor_type`]). The session calls this with its current map
    /// before every drive, so a late 0x0E lands before the next step.
    pub fn set_actor_types(&mut self, types: &std::collections::HashMap<u32, u8>) {
        self.actor_types = types.clone();
    }

    /// The retail entity Type byte (ent+0xEE) the 0x5B/0x66 gate applies to
    /// `actor`: 0x5B loads only for Type {1,2,7,8}, 0x66 only for {0,1,6}.
    /// Resolution stays
    /// in the hold-actor space: the local player is CHAR_PC (Type 0), the
    /// event-entity selector and the default-handler fallback resolve to the
    /// running scene's actor (falling back to the speaker's target index when
    /// no scene is attached), and a literal server id resolves to its target
    /// index. Anything else — party/alliance selectors, an entity the host
    /// never 0x0E'd — is Type 0, retail's value when the entity has no
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
    /// an [`EventCue::ActorMove`]; the VM never measures a move itself.
    pub fn hold_move(&mut self, actor: ActorLookup, units: f32) {
        let actor = self.resolve_hold_actor(actor);
        self.move_holds.retain(|h| h.actor != actor);
        self.move_holds.push(MoveHold {
            actor,
            remaining_units: units,
        });
    }

    /// True while a host-armed move hold for `actor` still has frames left.
    fn move_running(&self, actor: ActorLookup) -> bool {
        let actor = self.resolve_hold_actor(actor);
        self.move_holds
            .iter()
            .any(|h| h.actor == actor && h.remaining_units > 0.0)
    }

    /// True while a host-armed hold for `(actor, key)` still has frames left,
    /// or a motion hold is still pending its renderer finish report.
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
        // The same-batch bridge: an action started by this VM's own loader in the
        // current batch, before the host armed its hold from the cue.
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

    /// True while a message frame is displayed and parked on its MESWAIT —
    /// retail's CliEventMessOpenFlag up (research/XiEvents/OpCodes/0x0023.md).
    /// The host shows the box exactly for this span: dismissal clears the flag,
    /// so the box hides until the next message opcode reopens it. A child
    /// request holding the open frame counts too (retail's one global flag is
    /// shared by every entity's VM).
    pub fn message_awaiting(&self) -> bool {
        self.open_frame_holder().is_some() || self.message_open == MESSAGE_OPEN_AWAITING
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
        if self.scene_cancelled {
            return StepResult::Cancelled;
        }
        // A new EventIdle tick: retail re-queries IsMovingAction from live render
        // state, so the same-pass bridge (this VM's own loader cues not yet armed
        // by the host) spans only the pass that emitted them.
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
        // so re-stepping here must not reach the opcode again.
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
            // and authored events always reach one. Ours can miss it, because
            // the opcodes it steps over blind include the ones that would have
            // moved a loop's condition along; without a budget that is a hung
            // client rather than a dropped scene.
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
                // rather than dividing; wrapping_div additionally spares us the
                // i32::MIN / -1 panic where x86 idiv would trap.
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
                // the x86 shift retail compiles to already does.
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
                        // clears both.
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
                OP_QUERYWAIT => {
                    if !self.selection_made {
                        return match self.pending_choice.clone() {
                            Some(choice) => StepResult::AwaitChoice(choice),
                            None => StepResult::AwaitMessageAck,
                        };
                    }
                    self.selection_made = false;
                    self.pending_choice = None;
                    if self.work_zone.lock().unwrap()[0] == CHOICE_CANCELLED {
                        self.finished = true;
                        return StepResult::Cancelled;
                    }
                    self.exec_pointer += 1;
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
                    // The gate runs before the key check: a refused load
                    // leaves no resource entry, so SetAction is a silent
                    // no-op — no cue, no hold, still the full advance.
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
                OP_WAITLOADSCHEDULER => {
                    let actor = ActorLookup(self.eventgetcode2(WAITLOADSCHEDULER_ACTOR1_OFS));
                    let key = self.fourcc_at(WAITLOADSCHEDULER_KEY_OFS);
                    if self.action_running(actor, key) {
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
                OP_EVENTHIDE => {
                    self.cues.push(EventCue::ActorHide {
                        target: ActorLookup(self.eventgetcode2(EVENTHIDE_TARGET_OFS)),
                        hide: self.byte_at(EVENTHIDE_FLAG_OFS) & EVENTHIDE_FLAG_MASK != 0,
                    });
                    self.advance(op);
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
                    // Retail rewrites underscores to spaces before the rename.
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
                OP_CLOSE_MAP => {
                    self.cues.push(EventCue::MapClose);
                    self.advance(op);
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
                        });
                    }
                    self.advance(op);
                }
                OP_RESTORE_CLOCK => {
                    self.cues.push(EventCue::ClockHold {
                        stop: false,
                        hour: None,
                    });
                    self.advance(op);
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
                // The actor-driven waits: retail yields only once the named
                // entity resolves and reports mid-load/mid-turn/mid-animation,
                // and this VM resolves no actors, so each takes its own
                // "no such entity" advance (research/XiEvents/OpCodes/0x0080.md,
                // 0x0076.md, 0x0099.md — 0x99 advances on every path).
                OP_LOADWAIT | OP_TURNCHECK | OP_ANIMWAIT => self.exec_pointer += LOADWAIT_SIZE,
                OP_EMOT => self.exec_pointer += EMOT_SIZE,
                OP_TRANSPAR => self.exec_pointer += TRANSPAR_SIZE,
                OP_PLAYANIM => self.exec_pointer += PLAYANIM_SIZE,
                OP_MAPLOAD | OP_MAPLOAD_KEEP => self.exec_pointer += MAPLOAD_SIZE,
                OP_MUSICREADWAIT | OP_YIELD => self.exec_pointer += YIELD_SIZE,
                OP_BITTEST => self.op_bit_test(op),
                // 0x007F is 0x25 QUERYWAIT with one difference: a cancelled
                // menu stores 255 and runs on rather than ending the event
                // (research/XiEvents/OpCodes/0x007F.md).
                OP_QUERYWAIT2 => {
                    if !self.selection_made {
                        return match self.pending_choice.clone() {
                            Some(choice) => StepResult::AwaitChoice(choice),
                            None => StepResult::AwaitMessageAck,
                        };
                    }
                    self.selection_made = false;
                    self.pending_choice = None;
                    if self.work_zone.lock().unwrap()[0] == CHOICE_CANCELLED {
                        self.work_zone.lock().unwrap()[0] = CHOICE_CANCELLED_QUERYWAIT2;
                    }
                    self.exec_pointer += 1;
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
                        // 0xB4 case 1: the PENDINGSTR table entry the +4 work
                        // operand selects becomes that work string; an out-of-
                        // range index reads slot 0, retail's clamp.
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
                        _ => {}
                    }
                    self.exec_pointer += width as usize;
                }
                // The retail two-flag gate (CliEventCancelSetFlag) is empirically
                // open on every event that uses these opcodes — the locked-in
                // cutscenes disarm and stay disarmed, plain conversations never
                // touch it — so model the flag directly.
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
        // Event 531's shape: the zone block's exact entry is END and the
        // owners carry the programs. A wildcard-only block and a block whose
        // exact entry is END are not owners.
        let master = block(vec![OP_END], vec![]);
        let mut owner_a = block(vec![OP_EVENTHIDE, 1, 0, 0, 0, 0, OP_END], vec![]);
        owner_a.actor = NPC_SERVER_ID;
        // Owner B's entry sits mid-bytecode, so the entry must be the block's
        // own offset, not the master's.
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
        // Owner A: 0x43 case 0 (send the tag, park) -> case 1 poll -> END.
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
        // Owner B: 0x43 case 0 (park) -> case 1 poll -> 0x47 case 0 reading
        // x/z/y from Work_Zone[2]/[3]/[4] (operands 4098/4099/4100) -> case 1
        // poll -> END.
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
        // Two owner blocks: A stores 0xDEAD into Work_Zone[3] (0x03 GET_STORE,
        // work operand 4099), then parks on a one-second wait; B copies
        // Work_Zone[3] into its own Work_Local[0], then parks the same way.
        // The master does an END. All three VMs must see 0xDEAD in
        // Work_Zone[3]: retail's Work_Zone is one shared cell per event, so a
        // child's write reaches the master and the siblings without a copy.
        const NPC2: u32 = 0x0100_02C6;
        const ONE_SECOND: u32 = WAIT_UNITS_PER_SEC as u32;
        let owner_a = vec![
            OP_GET_STORE,
            0x03,
            0x10, // 4099: Work_Zone[3]
            0x00,
            0x80, // references[0]
            OP_WAIT,
            0x01,
            0x80, // wait references[1]
            OP_END,
        ];
        let owner_b = vec![
            OP_GET_STORE,
            0x00,
            0x00, // Work_Local[0]
            0x03,
            0x10, // Work_Zone[3]
            OP_WAIT,
            0x01,
            0x80, // wait references[1]
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

    /// 0x15 guards on *both* operands, so a zero numerator stores 0 rather than
    /// dividing, and a zero denominator never reaches the divide.
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

    /// The one-operand family. 0x0B is the loop counter that, while it was only
    /// being skipped by width, left ~1200 corpus events spinning until the
    /// opcode budget killed them (kuluu-cjct).
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

    /// 0x19 swaps the two slots rather than storing a computed value.
    #[test]
    fn endian_swap_exchanges_both_slots() {
        let data = vec![OP_SWAP, DST[0], DST[1], 0x01, 0x10, OP_END];
        let mut e = vm(data, vec![]);
        e.work_zone.lock().unwrap()[0] = 11;
        e.work_zone.lock().unwrap()[1] = 22;

        assert_eq!(e.step(), StepResult::Done);
        assert_eq!((e.work_zone(0), e.work_zone(1)), (22, 11));
    }

    /// 0x3C addresses a bit array: operand 3 is the flat bit index, so `>> 5`
    /// picks the slot and `& 0x1F` the bit within it, and operand 5 bounds the
    /// array (research/XiEvents/OpCodes/0x003C.md).
    #[test]
    fn bitarray_set_addresses_slot_and_bit() {
        // Bit 33 == slot 1, bit 1. Bound 2 admits it; bound 1 does not.
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
    /// answered and every fade snapped. The `0x1C` duration goes through
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

    /// A zero-length wait still yields once — retail sets RetFlag before testing
    /// the timer — so a scene can never spin through one without the host.
    /// Expiry is strictly `< 0.0` as in retail, so it takes a real (nonzero)
    /// slice of host clock to clear, never the same instant it was armed.
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

    #[test]
    fn running_off_the_end_is_done_and_flagged() {
        // A non-jumping, non-yield opcode (0x42, size 1) then off the end.
        let mut e = vm(vec![0x42], vec![]);
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

    #[test]
    fn loadextscheduler_family_advances_by_size_with_an_empty_key() {
        // An empty key emits no cue but still advances the full width; a real
        // key's cue is pinned by the dedicated tests below.
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

    #[test]
    fn loadextscheduler_emits_ext_cue_and_advances_15() {
        const FILE_OPERAND: u32 = 5; // band 0 -> base 32104
        let mut o = REF0.to_vec(); // file @1 -> References[0]
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(b"abcd");
        // The 0x5B gate loads only for entity Type {1,2,7,8}; the bare test
        // VM's event entity has no Type data, so install an accepted one under
        // the vm() helper's speaker index (5).
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
        const PACKAGE: u32 = 20; // the Sandy scene's Tpc package
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
        const PACKAGE: u32 = TPC_PACKAGE_OUT_OF_RANGE; // at the limit: loads nothing
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

    /// The 0x5B gate: ReadEventMotionRes loads only for entity Type
    /// {1,2,7,8}. One accepted
    /// and one refused Type, pinned on the cue and on the same-batch hold a
    /// following 0x53 parks on.
    #[test]
    fn loadextscheduler_gates_on_the_entity_type() {
        const FILE_OPERAND: u32 = 5;
        let mut o = REF0.to_vec();
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(b"abcd");
        let types = |t: u8| {
            let mut m = std::collections::HashMap::new();
            m.insert(5u32, t); // the vm() helper's speaker index
            m
        };
        // Accepted (Type 2, a standard-model NPC): the cue lands and a 0x53 in
        // the same pass parks on the same-batch start.
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
            "an accepted 0x5B must arm the same-batch hold"
        );
        // Refused (Type 0, the no-back-ptr value): no cue, and a 0x53 in the
        // same pass falls through to END.
        assert!(
            cues_of_with_types(OP_LOADEXTSCHEDULER, &o, vec![FILE_OPERAND], &types(0)).is_empty()
        );
        assert!(
            !wait_after_loader_parks(OP_LOADEXTSCHEDULER, &o, &types(0)),
            "a refused 0x5B must arm no hold"
        );
    }

    /// The 0x66 gate: ReadTpcEventMotionRes loads only for entity Type
    /// {0,1,6}.
    #[test]
    fn loadextscheduler2_gates_on_the_entity_type() {
        const PACKAGE: u32 = 20;
        let mut o = REF0.to_vec();
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(&LOOKUP_EVENT_ENTITY.to_le_bytes());
        o.extend_from_slice(b"abcd");
        let types = |t: u8| {
            let mut m = std::collections::HashMap::new();
            m.insert(5u32, t); // the vm() helper's speaker index
            m
        };
        // Accepted (Type 6, a standard-model mob): the cue lands and a 0x53 in
        // the same pass parks on the same-batch start.
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
            "an accepted 0x66 must arm the same-batch hold"
        );
        // Refused (Type 2, a standard-model NPC): no cue, no hold.
        assert!(cues_of_with_types(OP_LOADEXTSCHEDULER2, &o, vec![PACKAGE], &types(2)).is_empty());
        assert!(
            !wait_after_loader_parks(OP_LOADEXTSCHEDULER2, &o, &types(2)),
            "a refused 0x66 must arm no hold"
        );
    }

    #[test]
    fn a_script_that_loops_forever_stops_instead_of_hanging() {
        // GOTO 0: the tightest loop the bytecode can express. GOTO is
        // implemented, so this must report as a spin, not as work to do.
        let mut e = vm(vec![OP_GOTO, 0x00, 0x00], vec![]);
        assert_eq!(e.step(), StepResult::Spun(OP_GOTO));
        // And it stays stopped rather than spinning again on the next tick.
        assert_eq!(e.step(), StepResult::Done);
    }

    #[test]
    fn scheduler_wait_family_falls_through_without_a_hold() {
        // Sizes are load-bearing: a wrong width lands mid-instruction. With no
        // host-armed hold the wait advances immediately; the hold path is pinned
        // by the dedicated tests below.
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
    fn mapschedulor_emits_the_zone_routine_cue() {
        // 0x2D starts the zone-level routine (kuluu resolves the key out of the
        // current zone's own model DAT); the key sits at @9 like the WAIT family's,
        // and both actors ride along for the host (research/XiEvents/OpCodes/0x002D.md).
        const ACTOR1: u32 = 0x010E_6032; // literal server id, resolves to itself
        let key: [u8; 4] = *b"abcd";
        let mut data = vec![OP_MAPSCHEDULOR];
        data.extend_from_slice(&ACTOR1.to_le_bytes()); // actor1 @1
        data.extend_from_slice(&0u32.to_le_bytes()); // actor2 @5
        data.extend_from_slice(&key); // key @9
        data.push(OP_END); // offset 13
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
            data.extend_from_slice(&0u32.to_le_bytes()); // actor1 @1 (retail's guard only)
            data.extend_from_slice(&0u32.to_le_bytes()); // actor2 @5
            data.extend_from_slice(&key); // key @9
            data.push(OP_END); // offset 13
            vm(data, vec![])
        };
        let mut e = program();
        assert_eq!(
            e.step(),
            StepResult::Done,
            "no zone hold: the wait falls through"
        );
        let mut e = program();
        e.hold_action(ActorLookup::ZONE, key, WAIT_UNITS_PER_SEC); // one second of frames
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the zone hold parks the wait"
        );
        e.tick(1.1); // 66 frames: past the one-second hold
        assert_eq!(
            e.step(),
            StepResult::Done,
            "an expired hold falls through to END"
        );
    }

    #[test]
    fn mapschedulor_and_its_wait_in_one_batch_bridge_until_the_cues_drain() {
        let key: [u8; 4] = *b"abcd";
        let mut data = vec![OP_MAPSCHEDULOR];
        data.extend_from_slice(&0u32.to_le_bytes()); // actor1 @1
        data.extend_from_slice(&0u32.to_le_bytes()); // actor2 @5
        data.extend_from_slice(&key); // key @9
        data.push(OP_WAITMAPSCHEDULOR); // offset 13
        data.extend_from_slice(&0u32.to_le_bytes()); // actor1 @14
        data.extend_from_slice(&0u32.to_le_bytes()); // actor2 @18
        data.extend_from_slice(&key); // key @22
        data.push(OP_END); // offset 26
        let mut e = vm(data, vec![]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the wait parks on the zone routine its own loader just started"
        );
        assert_eq!(e.take_cues().len(), 1);
        // The host arms the hold from the drained cue; it takes over from the bridge.
        e.hold_action(ActorLookup::ZONE, key, WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the armed hold keeps the wait parked"
        );
    }

    #[test]
    fn waitschedulor_holds_while_action_runs_then_falls_through() {
        const ACTOR: u32 = 0x010E_6032; // literal server id, resolves to itself
        let key: [u8; 4] = *b"abcd";
        let mut data = vec![OP_WAITSCHEDULOR];
        data.extend_from_slice(&ACTOR.to_le_bytes()); // actor1 @1
        data.extend_from_slice(&0u32.to_le_bytes()); // actor2 @5 (unused by the hold)
        data.extend_from_slice(&key); // key @9
        data.push(OP_END); // offset 13
        let mut e = vm(data, vec![]);
        e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC); // one second of frames
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
        e.tick(0.6); // 1.1 s total: the one-second hold has expired
        assert_eq!(
            e.step(),
            StepResult::Done,
            "an expired hold falls through to END"
        );
    }

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
        // No hold armed: the wait advances immediately instead of parking.
        assert_eq!(e.step(), StepResult::Done);
    }

    fn waitschedulor_program(actor: u32, key: [u8; 4]) -> Vec<u8> {
        let mut data = vec![OP_WAITSCHEDULOR];
        data.extend_from_slice(&actor.to_le_bytes()); // actor1 @1
        data.extend_from_slice(&0u32.to_le_bytes()); // actor2 @5 (unused by the hold)
        data.extend_from_slice(&key); // key @9
        data.push(OP_END); // offset 13
        data
    }

    #[test]
    fn waitschedulor_parks_on_a_pending_hold_until_the_renderer_reports() {
        const ACTOR: u32 = 0x010E_6032; // literal server id, resolves to itself
        let key: [u8; 4] = *b"kue0";
        let mut e = vm(waitschedulor_program(ACTOR, key), vec![]);
        e.hold_action_pending(ActorLookup(ACTOR), key);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "a pending 0x2C hold parks the wait"
        );
        // The pending hold has no length: no amount of host clock releases it.
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

    #[test]
    fn release_action_hold_is_a_noop_when_nothing_is_pending() {
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut e = vm(waitschedulor_program(ACTOR, key), vec![]);
        // A stray report (the routine was stopped or the event ended) must not
        // panic or touch a timed hold for the same pair.
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
        e.tick(1.1); // past the one-second timed hold, no release ever arrives
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
        e.tick(1.1); // past the one-second timed hold, which the pending replaced
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
        data.extend_from_slice(&REF0); // file @1 -> references[0]
        data.extend_from_slice(&actor.to_le_bytes()); // actor1 @3
        data.extend_from_slice(&0u32.to_le_bytes()); // actor2 @7
        data.extend_from_slice(&key); // key @11
        data.push(OP_WAITSCHEDULOR); // offset 15
        data.extend_from_slice(&actor.to_le_bytes()); // actor1 @16
        data.extend_from_slice(&0u32.to_le_bytes()); // actor2 @20
        data.extend_from_slice(&key); // key @24
        data.push(OP_END); // offset 28
        data
    }

    /// The 0x5B gate's accepted Type for the bridge tests' literal actor,
    /// installed under the target index retail's GetActorIndex resolves it to.
    fn bridge_types(actor: u32) -> std::collections::HashMap<u32, u8> {
        let mut m = std::collections::HashMap::new();
        m.insert(actor & 0x3FF, 2u8);
        m
    }

    #[test]
    fn loadextscheduler_and_its_wait_in_one_batch_bridge_until_the_cues_drain() {
        const ACTOR: u32 = 0x010E_6032; // literal server id, resolves to itself
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

    #[test]
    fn loadextscheduler_bridge_hands_off_to_the_host_armed_hold() {
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        let mut e = vm(loadextscheduler_then_wait_program(ACTOR, key), vec![5]);
        e.set_actor_types(&bridge_types(ACTOR));
        assert_eq!(e.step(), StepResult::Waiting);
        // The host arms the hold from the drained cue; it takes over from the bridge.
        assert_eq!(e.take_cues().len(), 1);
        e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the armed hold keeps the wait parked"
        );
        e.tick(1.1); // 66 frames: past the one-second hold
        assert_eq!(
            e.step(),
            StepResult::Done,
            "an expired hold falls through to END"
        );
    }

    #[test]
    fn waitloadscheduler_reads_actor_at_3_and_key_at_11() {
        const ACTOR: u32 = 0x010E_6032;
        let key: [u8; 4] = *b"abcd";
        // 0x55 layout: file @1, actor1 @3, actor2 @7, key @11.
        let mut data = vec![OP_WAITLOADSCHEDULER];
        data.extend_from_slice(&0u16.to_le_bytes()); // file @1 (unused by the hold)
        data.extend_from_slice(&ACTOR.to_le_bytes()); // actor1 @3
        data.extend_from_slice(&0u32.to_le_bytes()); // actor2 @7
        data.extend_from_slice(&key); // key @11
        data.push(OP_END); // offset 15
        let mut e = vm(data, vec![]);
        e.hold_action(ActorLookup(ACTOR), key, WAIT_UNITS_PER_SEC);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "0x55 must read actor @3 and key @11"
        );
    }

    #[test]
    fn message_then_meswait_yields_then_resumes() {
        // 0x1D msg (ref index 0x8000 -> references[0]=900), then 0x23 MESWAIT, END.
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
        // ack that would dismiss the displayed frame.
        assert_eq!(e.step(), StepResult::Waiting);
        e.dismiss_message();
        assert_eq!(e.step(), StepResult::Done);
    }

    #[test]
    fn params_flow_through_message_and_choice() {
        // The trigger packet's num[8] must ride along on both yield kinds.
        let params = vec![7, -1, 42];
        let data = vec![
            OP_MESSAGE,
            0x00,
            0x80, // msg: References[0]=900
            OP_MESWAIT,
            OP_QUERY,
            0x00,
            0x80,
            0x01,
            0x80,
            0x00,
            0x00, // QUERY(msg=ref0, default=ref1)
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

    #[test]
    fn if_equal_case1_branches_to_target() {
        // case 1: jump to target when references[0]==references[0]. Layout:
        // [0]=0x02 op, [1..3]=v1 ref idx 0x8000, [3..5]=v2 ref idx 0x8000,
        // [5]=kind 1, [6..8]=val3=9 (absolute into EventData),
        // [8]=0xFF(skip), [9]=END.
        let data = vec![
            OP_IF, 0x00, 0x80, 0x00, 0x80, 0x01, 0x09, 0x00, 0xFF, OP_END,
        ];
        let mut e = vm(data, vec![42]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 9);
    }

    /// The taken-branch target is absolute into EventData, not relative to the
    /// IF: with the IF at 5 and val3 = 20, retail lands on 20; a relative read
    /// would land on 25, past this program's END.
    #[test]
    fn if_taken_branch_target_is_absolute_into_event_data() {
        // [0..5] WZ[0] = ref[0] (=5); [5..13] IF case 1 (jump when equal),
        // v1 = WZ[0], v2 = ref[1] (=5), val3 = 20; [13..20) fall-through poison;
        // [20..23] WZ[2] = 1; [23] END.
        let mut data = seed().to_vec();
        data.extend_from_slice(&[
            OP_IF, DST[0], DST[1], SRC[0], SRC[1], 0x01, // case 1: jump when equal
            0x14, // val3 = 20 (absolute into EventData)
            0x00,
        ]);
        data.extend_from_slice(&[0xFF; 7]); // 13..20: fall-through poison
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

    #[test]
    fn unimplemented_jump_opcode_stops() {
        // 0x44 is a jumping opcode we don't implement; it must not be skipped by
        // size (that would desync ExecPointer), so the VM stops.
        const OP_UNIMPLEMENTED_JUMP: u8 = 0x44;
        assert!(OPCODE_META[OP_UNIMPLEMENTED_JUMP as usize].jumps);
        let mut e = vm(vec![OP_UNIMPLEMENTED_JUMP, 0, 0, 0, 0, 0, 0], vec![]);
        assert_eq!(e.step(), StepResult::Unimplemented(OP_UNIMPLEMENTED_JUMP));
        assert_eq!(e.exec_pointer(), 0);
    }

    /// The actor-driven waits all reduce to retail's own "no such entity"
    /// advance here, and the width is load-bearing: each carries an actor
    /// lookup the VM steps over blind.
    #[test]
    fn actor_early_exit_opcodes_skip_by_size_and_continue() {
        for (op, size) in [
            (OP_LOADWAIT, LOADWAIT_SIZE),
            (OP_TURNCHECK, LOADWAIT_SIZE),
            (OP_ANIMWAIT, LOADWAIT_SIZE),
            (OP_EMOT, EMOT_SIZE),
            (OP_TRANSPAR, TRANSPAR_SIZE),
            (OP_MAPLOAD, MAPLOAD_SIZE),
            (OP_MAPLOAD_KEEP, MAPLOAD_SIZE),
            (OP_MUSICREADWAIT, YIELD_SIZE),
            (OP_YIELD, YIELD_SIZE),
            (OP_PLAYANIM, PLAYANIM_SIZE),
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

    #[test]
    fn unknown_nonjump_opcode_skipped_by_size() {
        // 0x30 (size 1, no jump/ret) is skipped; reaches END.
        let mut e = vm(vec![0x30, 0x30, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
    }

    #[test]
    fn cancel_flag_disarms_and_rearms() {
        // Armed at event start; 0x42 (event 503's second opcode) locks the
        // event in, and 0x2E re-arms it for interactive beats.
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
        ExtSchedulerMotion, FourCc, TpcMotionPackages, SCHEDULER_DURATION_FROM_DAT,
        SCHEDULER_FADE_DAT_ID, SCHEDULER_TAG_FADE_IN, SCHEDULER_TAG_FADE_OUT,
        TPC_PACKAGE_OUT_OF_RANGE,
    };

    /// Run one choreography opcode (padded to its documented width) to END and
    /// return the cues it emitted.
    fn cues_of(op: u8, operands: &[u8], references: Vec<u32>) -> Vec<EventCue> {
        cues_of_with_types(op, operands, references, &std::collections::HashMap::new())
    }

    /// [`cues_of`] with the entity Type table the 0x5B/0x66 gate reads
    /// installed before the step: a bare VM has no Type data, and an absent
    /// entry is Type 0 — retail's no-back-ptr value, which refuses 0x5B.
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

    /// True when a 0x53 after `loader` (with `loader_operands`, `loader`'s
    /// width, and `FILE_OFS`-style padding) parks on the loader's same-batch
    /// start instead of falling through to END.
    fn wait_after_loader_parks(
        loader: u8,
        loader_operands: &[u8],
        types: &std::collections::HashMap<u32, u8>,
    ) -> bool {
        let mut data = vec![loader];
        data.extend_from_slice(loader_operands);
        // 0x53: actor1 (event entity) @+1, key "abcd" @+9.
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

    /// 0x2C's third operand is an ASCII action key, not a numeric id.
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
    /// lookup at +2 and the cue is event-scoped either way.
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

    /// 0x22 carries no target operand: it always hides/shows the event's own entity.
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

    /// 0xC8's three work operands resolve through the References store; only
    /// the LOBYTE of the third is the tutorial flag.
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

    /// 0x8B carries the marker's zone id and milli-unit position in work slots
    /// and its 16-byte name inline; underscores become spaces.
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
        data.extend_from_slice(&[0x00, 0x80]); // References[0]=900
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
        // open flag up); the map still does not close.
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

    /// 0x67/0x68 stage the whole-HUD hide/show; 0x67's operands are retail's event-message
    /// presets, so they carry no cue payload.
    #[test]
    fn hud_opcodes_emit_the_hide_and_show_cues() {
        // Southern San d'Oria event 503 authors these HIDE_HUD operand bytes.
        assert_eq!(
            cues_of(OP_HIDE_HUD, &[0x91, 0x80, 0xA7, 0x81], vec![]),
            [EventCue::HudHide { hide: true }]
        );
        assert_eq!(
            cues_of(OP_SHOW_HUD, &[], vec![]),
            [EventCue::HudHide { hide: false }]
        );
    }

    /// 0x77's hour operand resolves through References and wraps the Vana'diel day; the
    /// sentinel means no time change.
    #[test]
    fn stop_clock_opcode_carries_the_authored_hour() {
        // Hour -> refs[1], weather -> refs[3] (the latter has no cue).
        let ops = [0x01u8, 0x80, 0x03, 0x80];
        assert_eq!(
            cues_of(OP_STOP_CLOCK, &ops, vec![0, 8, 0, 1]),
            [EventCue::ClockHold {
                stop: true,
                hour: Some(8)
            }]
        );
        assert_eq!(
            cues_of(OP_STOP_CLOCK, &ops, vec![0, 30, 0, 1]),
            [EventCue::ClockHold {
                stop: true,
                hour: Some(6)
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
                hour: None
            }]
        );
    }

    /// 0x46 case 1 takes the camera, case 0 gives it back; case 2 only queries
    /// the current state, so it stages nothing.
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

    /// 0x5D's first operand is a volume *table index*, its second a frame count.
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

    /// The send-tag pair: case 0 sends the tag with EndPara = Work_Zone[1] and holds
    /// execution on the following case-1 poll; ack_server releases it past the
    /// pair (research/XiEvents/OpCodes/0x0043.md).
    #[test]
    fn sendtag_pair_holds_until_ack() {
        // WZ[1] = refs[1], then the `43 00` / `43 01` pair.
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

    /// The position-tag pair: case 0 scales its work-slot raw values into the c2s payload
    /// (coordinates times 0.001; heading through LSB's radianToRotation scale)
    /// and holds on the following case-1 poll until ack_server releases it
    /// (research/XiEvents/OpCodes/0x0047.md).
    #[test]
    fn xzy_tag_pair_scales_work_slots_and_holds_until_ack() {
        // refs[1..5] hold the raw work values: x, y, z, dir (1/4096-turn units).
        let mut data = vec![OP_EVENTPOSSET, 0x00];
        for i in 1u16..=4 {
            // refs[i] operand: index with the REFERENCE_FLAG marker byte.
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
                // A quarter turn lands at 63.998 on the wire's 0..=255 scale:
                // retail's f32 literals fall just short of exactly a quarter
                // and the conversion truncates rather than rounds.
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
    /// the updated value (research/XiPackets/world/server/0x005C).
    #[test]
    fn pending_num_lands_while_held_and_ack_runs_on() {
        // [0..5) WZ[1] = refs[0]; [5..7) 43 00 send+hold; [7..9) 43 01 poll;
        // [9..17) IF case 1: jump when WZ[2] == refs[1], target absolute 20;
        // [17..20) fall-through poison; [20..23) WZ[3] = 1; [23] END.
        let mut data = vec![OP_GET_STORE, 0x01, 0x10, REF0[0], REF0[1]];
        data.extend_from_slice(&[OP_SENDTAG, 0x00, OP_SENDTAG, 0x01]);
        data.extend_from_slice(&[
            OP_IF, 0x02, // v1 = WZ slot 2
            0x10, SRC[0], // v2 = refs[1]
            SRC[1], 0x01, // case 1: jump when equal
            0x14, // val3 = 20 (absolute into EventData)
            0x00,
        ]);
        data.extend_from_slice(&[0xFF; 3]);
        data.extend_from_slice(&[OP_SET_ONE, 0x03, 0x10]);
        data.push(OP_END);
        let mut e = vm(data, vec![42, 7]);

        assert_eq!(
            e.step(),
            StepResult::AwaitServerAck(PendingTag::SendTag { end_para: 42 })
        );
        // The PENDINGNUM write lands while the tag is still held: num[0] is
        // Work_Zone[2], where this script's loop condition lives.
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

    #[test]
    fn reqset_spawns_child_on_target_block_at_tag_index() {
        // Master: REQSET the NPC's tag 1 at priority 0, then END. The NPC block
        // hides itself and ends.
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
        // The child's first frame runs on the next step; its cue bubbles up with
        // the NPC as event entity.
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(
            e.take_cues(),
            [EventCue::ActorHide {
                target: ActorLookup(NPC_SERVER_ID),
                hide: true,
            }]
        );
    }

    #[test]
    fn reqwait_holds_until_target_stack_drains_at_or_below_priority() {
        // Master: REQSET the NPC's tag 1 at priority 3, then REQWAIT priority 3.
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(3, NPC_SERVER_ID, 1));
        master.push(OP_REQWAIT);
        master.extend_from_slice(&reqwait_operands(3, NPC_SERVER_ID));
        master.push(OP_END);
        // The NPC parks on a ten-second timed wait.
        const TEN_SECONDS: u32 = 10 * WAIT_UNITS_PER_SEC as u32;
        let npc = vec![OP_WAIT, REF0[0], REF0[1]];

        let mut e = scene_vm_refs(master, npc.clone(), vec![TEN_SECONDS]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the REQWAIT parks on its own push"
        );
        // The child arms its wait on the step after the push; the master stays parked.
        assert_eq!(e.step(), StepResult::Waiting);
        e.tick(5.0);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "halfway through the child's wait"
        );
        e.tick(5.1); // 10.1 s: past the ten-second wait
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the drained stack releases the REQWAIT"
        );

        // A numerically higher priority on the stack does not hold a lower
        // REQWAIT byte.
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

    #[test]
    fn reqew_pushes_then_waits_for_that_tag() {
        // Master: REQEW the NPC's tag 1, then END. The NPC parks on a one-second
        // timed wait.
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
        // The child arms its wait on this step; the master re-evaluates and stays held.
        assert_eq!(e.step(), StepResult::Waiting);
        e.tick(1.5); // past the one-second wait
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the tag leaves the stack when its request ends"
        );
    }

    #[test]
    fn lower_priority_number_preempts_and_the_other_resumes() {
        // One NPC block, four tags: tag 2 is a one-second wait, tag 3 hides
        // and ends. The master REQSETs both; the lower number runs first, and
        // the other starts only when it drains.
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(9, NPC_SERVER_ID, 2));
        master.push(OP_REQSET);
        master.extend_from_slice(&reqset_operands(1, NPC_SERVER_ID, 3));
        master.push(OP_END);
        // [0..3) tag 2: one-second wait; [3..10) tag 3: hide + END.
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
        // Neither request has run yet: the lower number goes first on the next
        // frame.
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
        e.tick(1.5); // past tag 0's wait
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the drained stack ends the event"
        );
    }

    #[test]
    fn stack_full_makes_reqset_yield() {
        // An NPC block with 17 placeholder entries, all starting at offset 0 of a
        // one-second wait; the master REQSETs tags 1..16 (sixteen pushes fill
        // the stack) and then tag 0, which must yield on the full stack — the
        // tag-0 no-op only applies while some slot is still zeroed.
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

    #[test]
    fn reqset_tag_zero_is_always_a_noop_on_a_non_full_stack() {
        // Retail's ReqSet walks all 16 ReqStack slots and returns 0 on any
        // matching TagNum, including the zeroed TagNum of unused slots, so a
        // REQSET of tag 0 spawns no child while the target's stack is not
        // full. The master REQSETs tag 0 on an idle NPC and must run straight
        // through to its END.
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

    #[test]
    fn placeholder_tag_entries_are_reqset_entry_points() {
        // The NPC block's second entry carries the placeholder event id; REQSET
        // by tag index still reaches it, because ReqSet indexes TagOffset
        // directly.
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

    #[test]
    fn npc_move_emits_actor_move_and_case1_holds_on_host_hold() {
        // Master: REQSET the NPC's tag 0, then END. The NPC sets its speed, walks
        // to a goal (case 0), and holds on the arrival test (case 1).
        // [0..3) speed = refs[0] * 0.1; [3..11) MOVE case 0 goal: x @2,
        // z @4 and y @6 as work operands, all through the References table
        // because a plain bytecode value is a work-store index; [11..13)
        // MOVE case 1; [13] END.
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
        // The child's first frame: speed lands, case 0 emits the move cue, and
        // with no host-armed hold case 1 falls through to END.
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
            }]
        );

        // With a host-armed move hold (copied into the child before its first
        // frame), case 1 parks until it expires.
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
        e.hold_move(ActorLookup(NPC_SERVER_ID), WAIT_UNITS_PER_SEC); // one second
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
        // The child's first frame opens its line: it surfaces to the host with
        // the message id from the NPC block's references table.
        let StepResult::AwaitMessage(m) = e.step() else {
            panic!("the child's dialog frame did not surface");
        };
        assert_eq!(m.message_id, MSG_ID);
        assert_eq!(
            m.speaker_index, None,
            "0x48 prints through the nameless path"
        );
        // While the frame is up, re-steps park: no second frame, and the
        // master's REQWAIT still holds because the child request has not drained.
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
        // The first child's line surfaces; the second parks on a dialog while
        // that frame is open and is dropped.
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
    /// their CodeMOVE2 pair, which stalled the event between E3 and E4.
    #[test]
    fn child_stopped_on_unrunnable_opcode_is_dropped_and_releases_the_reqwait() {
        let mut master = vec![OP_REQSET];
        master.extend_from_slice(&reqset_operands(3, NPC_SERVER_ID, 1));
        master.push(OP_REQWAIT);
        master.extend_from_slice(&reqwait_operands(3, NPC_SERVER_ID));
        master.push(OP_END);
        // LOADROOM with an undocumented sub stops the VM (sub_size has no width
        // for it), so the child can never drain on its own.
        let npc = vec![OP_LOADROOM, 0xFF, 0, 0, OP_END];

        let mut e = scene_vm_refs(master, npc, vec![]);
        assert_eq!(
            e.step(),
            StepResult::Waiting,
            "the master ends; the child is queued behind its REQWAIT"
        );
        // The child's first frame stops on 0x75: it must be dropped so the
        // REQWAIT sees an empty stack and the event can finish.
        assert_eq!(
            e.step(),
            StepResult::Done,
            "the unrunnable child is dropped; its REQWAIT releases"
        );
    }

    /// CodeMOVE2 (0x5A) is retail's uncalibrated twin of MOVE (0x1F): case 0
    /// emits the walk cue, case 1 holds on the host-armed move hold and
    /// advances exactly two bytes so the next opcode still runs. Event 503's
    /// party children open their tag[2] walk-in with this pair.
    #[test]
    fn code_move2_walks_like_move_on_an_npc_child() {
        // [0..3) speed = refs[0] * EVENT_SPEED_SCALE; [3..11) CodeMOVE2 case 0
        // goal: x @2, z @4 and y @6 through the References table; [11..13)
        // CodeMOVE2 case 1; then an EVENTHIDE cue that only runs if case 1
        // advanced two bytes instead of the table's widest eight.
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
        // No host-armed hold: case 1 falls through and the hide cue proves the
        // two-byte advance landed on the next instruction.
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
                },
                EventCue::ActorHide {
                    target: ActorLookup(NPC_SERVER_ID),
                    hide: true,
                },
            ]
        );

        // With a host-armed move hold (copied into the child before its first
        // frame), case 1 parks until it expires.
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
        e.hold_move(ActorLookup(NPC_SERVER_ID), WAIT_UNITS_PER_SEC); // one second
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
        data.extend_from_slice(&3u16.to_le_bytes()); // dest slot @2
        data.extend_from_slice(&literal); // literal @4
        data.push(OP_END); // offset 20
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
        data.extend_from_slice(&3u16.to_le_bytes()); // dest slot @2
        data.extend_from_slice(&0x8007u16.to_le_bytes()); // refs[7] @4
        data.push(OP_END); // offset 6
        let mut pending = [[0u8; 16]; 4];
        pending[2] = *b"pending-two\0\0\0\0\0";
        let mut e = vm(data, vec![0, 0, 0, 0, 0, 0, 0, 2]);
        e.apply_pending_str(&pending);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.work_local_str[3], pending[2]);
    }

    /// 0xB4 case 1 with a literal index past the four-entry table: retail
    /// lands on entry 0, not the clamped last entry.
    #[test]
    fn window_case_one_out_of_range_index_reads_slot_zero() {
        let mut data = vec![OP_WINDOW, 0x01];
        data.extend_from_slice(&3u16.to_le_bytes()); // dest slot @2
        data.extend_from_slice(&9u16.to_le_bytes()); // literal index 9 @4
        data.push(OP_END); // offset 6
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
        data.extend_from_slice(&5u16.to_le_bytes()); // dest slot @2
        data.extend_from_slice(&literal); // literal @4
        data.extend_from_slice(&[OP_NAMESET, 0x00]); // offset 20
        data.extend_from_slice(&5u16.to_le_bytes()); // source slot @22
        data.push(OP_END); // offset 24
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

    /// setworkstrofs refuses both stores: a References-flagged operand is
    /// read-only, and the write bound is slot 64 even though the int view runs
    /// to 80 (research/XiEvents/Event VM Functions.md).
    #[test]
    fn window_case_zero_refuses_ref_flagged_and_over_bound_destinations() {
        let literal: [u8; 16] = *b"Sajj'aka\0\0\0\0\0\0\0\0";
        for dest in [0x8003u16, 64] {
            let mut data = vec![OP_WINDOW, 0x00];
            data.extend_from_slice(&dest.to_le_bytes()); // dest slot @2
            data.extend_from_slice(&literal); // literal @4
            data.push(OP_END); // offset 20
            let mut e = vm(data, vec![0, 0, 0, 0]);
            assert_eq!(e.step(), StepResult::Done);
            assert_eq!(
                e.work_local_str, [[0u8; 16]; WORK_LOCAL_LEN],
                "dest 0x{dest:04X} must not store"
            );
        }
    }
}
