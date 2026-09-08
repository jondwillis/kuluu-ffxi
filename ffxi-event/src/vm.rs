use ffxi_dat::event_dat::EventBlock;

use crate::cue::{
    dat_id_helper, ActorLookup, EventCue, FourCc, MUSIC_VOLUME_MAX, SCHEDULER_DAT_ID_BASE,
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

/// Outcome of running the VM until it next needs the host (one `XiEvent::EventIdle`
/// tick: opcodes execute until `RetFlag`).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// and steps again. Only the pure timers yield here — an actor-gated wait
    /// would block on state this VM never models, turning a dropped scene into a
    /// hung client.
    Waiting,
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
pub(crate) const OP_MESWAIT: u8 = 0x23;
pub(crate) const OP_QUERY: u8 = 0x24;
pub(crate) const OP_QUERYWAIT: u8 = 0x25;
const OP_REQSET: u8 = 0x27;
const OP_REQSET_CHECKED: u8 = 0x28;
const OP_REQSET_PRIORITY: u8 = 0x29;
const OP_REQWAIT: u8 = 0x2A;
const OP_LOADEXTSCHEDULER: u8 = 0x5B;
const OP_LOADEXTSCHEDULER2: u8 = 0x66;
const OP_SCHEDULOR: u8 = 0x2C;
const OP_LOADEVENTSCHEDULER2: u8 = 0x45;
const OP_DEFCAMERA: u8 = 0x46;
const OP_EVENTHIDE: u8 = 0x4E;
const OP_MUSICVOLUME: u8 = 0x5D;
const OP_WAITSCHEDULOR: u8 = 0x53;
const OP_WAITMAPSCHEDULOR: u8 = 0x54;
const OP_WAITLOADSCHEDULER: u8 = 0x55;
const OP_CHOCOBO: u8 = 0x7E;
const OP_SETBITWORK: u8 = 0x40;
const OP_GETBITWORK: u8 = 0x41;
const OP_SENDTAG: u8 = 0x43;
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

const MESSAGE_OPEN_NONE: u8 = 0;
const MESSAGE_OPEN_AWAITING: u8 = 1;
// CliEventMessOpenFlag = 2 is the invalid-open state MESWAIT force-cancels on
// (XiEvents OpCodes/0x0023.md).
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
const DEFCAMERA_CASE_OFS: usize = 1; // 0x0046
const DEFCAMERA_CASE_UNLOCK: u8 = 0;
const DEFCAMERA_CASE_LOCK: u8 = 1;
const EVENTHIDE_FLAG_OFS: usize = 1; // 0x004E
const EVENTHIDE_FLAG_MASK: u8 = 1;
const EVENTHIDE_TARGET_OFS: usize = 2;
const MUSICVOLUME_LEVEL_OFS: usize = 1; // 0x005D
const MUSICVOLUME_FADE_OFS: usize = 3;
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
const WORK_ZONE_LEN: usize = 96;
const WORK_ZONE_BASE: u32 = 4096;
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
const OPCODE_BUDGET_PER_STEP: u32 = 100_000;

/// `XiEvent` runtime for a single event, simplified to the linear+jump+message
/// flow (the full 16-entry priority `ReqStack` is a Stage 2 concern). Mirrors the
/// fields the implemented opcodes touch.
pub struct EventVm {
    event_data: Vec<u8>,
    references: Vec<u32>,
    work_local: [u32; WORK_LOCAL_LEN],
    work_zone: [u32; WORK_ZONE_LEN],
    exec_pointer: usize,
    jump_table: [u16; JUMP_STACK_LEN],
    jump_index: usize,
    speaker_index: u16,
    /// Event numeric parameters from the trigger packet — see
    /// [`EventMessage::params`].
    params: Vec<i32>,
    /// `CliEventMessOpenFlag`: 0 none, 1 awaiting dismissal, 2 invalid.
    message_open: u8,
    pending_message: Option<EventMessage>,
    pending_choice: Option<EventChoice>,
    selection_made: bool,
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
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Wait {
    remaining_units: f32,
    advance: usize,
}

/// `WaitTime` decrements by `GetFrameDelay()`, which counts 1/60ths of a
/// second: research/XiEvents OpCodes/0x005A.md scales it by `0.016666668` and
/// 0x0031.md divides it by 60.0 for per-second motion.
const WAIT_UNITS_PER_SEC: f32 = 60.0;

/// `0x6F` SLEEP authors no operand and loads this fixed duration
/// (research/XiEvents OpCodes/0x006F.md).
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
        Some(Self {
            event_data: block.event_data.clone(),
            references: block.references.clone(),
            work_local: [0; WORK_LOCAL_LEN],
            work_zone: [0; WORK_ZONE_LEN],
            exec_pointer,
            jump_table: [0; JUMP_STACK_LEN],
            jump_index: 0,
            speaker_index,
            params,
            message_open: MESSAGE_OPEN_NONE,
            pending_message: None,
            pending_choice: None,
            selection_made: false,
            cues: Vec::new(),
            finished: false,
            ran_past_end: false,
            wait: None,
            oob_reads: std::cell::Cell::new(0),
        })
    }

    /// Clear the open-dialog flag after the player dismisses a message, so the
    /// next [`step`](Self::step) advances past MESWAIT.
    pub fn dismiss_message(&mut self) {
        self.message_open = MESSAGE_OPEN_NONE;
    }

    /// Mark the open message invalid so the next MESWAIT force-cancels the
    /// event (XiEvents OpCodes/0x0023.md) — the Esc-on-message path.
    pub fn cancel_message(&mut self) {
        self.message_open = MESSAGE_OPEN_INVALID;
    }

    /// Record the player's menu selection (0-based, or [`u32::MAX`] to cancel)
    /// into `Work_Zone[0]` — the slot QUERYWAIT writes and subsequent `if`
    /// opcodes branch on — so the next [`step`](Self::step) advances past
    /// QUERYWAIT.
    pub fn select_choice(&mut self, index: Option<u32>) {
        self.work_zone[0] = index.unwrap_or(CHOICE_CANCELLED);
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
        std::mem::take(&mut self.cues)
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
        self.work_zone.get(index).copied().unwrap_or(0) as i32
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
    }

    /// Run opcodes until the VM yields (one `EventIdle` tick).
    pub fn step(&mut self) -> StepResult {
        if self.finished {
            return StepResult::Done;
        }
        if self.wait.is_some() {
            return StepResult::Waiting;
        }
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
                return StepResult::Done;
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
            match op {
                OP_END => {
                    self.finished = true;
                    return StepResult::Done;
                }
                // 0x21 sets EventExecEnd, which stops XiEvent::EventIdle from
                // running the program again — the event is over (XiEvents
                // OpCodes/0x0021.md).
                OP_EXECEND => {
                    self.finished = true;
                    return StepResult::Done;
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
                // bit index, so `>> 5` picks the slot and `& 0x1F` the bit, and
                // operand 5 bounds the array (XiEvents OpCodes/0x003C.md).
                OP_BITARRAY_SET => {
                    let bit = self.getworkofs(3, 0);
                    let slot = bit >> 5;
                    if slot < self.getworkofs(5, 0) {
                        let prev = self.getworkofs(1, slot);
                        self.setworkofs(1, 1i32.wrapping_shl((bit & 0x1F) as u32) | prev, slot);
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
                // is reachable (research/XiEvents OpCodes/0x0070.md).
                OP_TURNWAIT => self.exec_pointer += 1,
                OP_SLEEP => return self.arm_wait(SLEEP_WAIT_UNITS, 1),
                OP_WAIT => {
                    let units = self.getworkofs(1, 0) as f32;
                    return self.arm_wait(units, 3);
                }
                // 0x43 asks the host to send the pending 0x05B tag to the server
                // and advances +2 on success. The actual mid-event send is a
                // session-level refinement; locally we advance so the script runs
                // on (XiEvents OpCodes/0x0043.md).
                OP_SENDTAG => self.exec_pointer += 2,
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
                        return StepResult::Done;
                    }
                    self.jump_index -= 1;
                    self.exec_pointer = self.jump_table[self.jump_index] as usize;
                }
                OP_MESSAGE => {
                    let message_id = self.getworkofs(MESSAGE_ID_OFS, 0) as u32;
                    self.open_message(message_id, Some(self.speaker_index));
                    self.advance(op);
                }
                OP_MESSAGE_ACTOR => {
                    let speaker = self.actor_index(self.eventgetcode2(ACTOR_LOOKUP_OFS));
                    let message_id = self.getworkofs(ACTOR_MESSAGE_ID_OFS, 0) as u32;
                    self.open_message(message_id, Some(speaker));
                    self.advance(op);
                }
                OP_MESSAGE_UNNAMED => {
                    let message_id = self.getworkofs(MESSAGE_ID_OFS, 0) as u32;
                    self.open_message(message_id, None);
                    self.advance(op);
                }
                // 0x49 resolves an actor into MESCASNAMEINDEX/MESTARNAMEINDEX but
                // prints through the nameless EventMessDecodePut, so the line
                // carries no speaker (research/XiEvents/OpCodes/0x0049.md).
                OP_MESSAGE_UNNAMED_ACTOR => {
                    let message_id = self.getworkofs(ACTOR_MESSAGE_ID_OFS, 0) as u32;
                    self.open_message(message_id, None);
                    self.advance(op);
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
                }
                OP_MESWAIT => match self.message_open {
                    MESSAGE_OPEN_NONE => self.exec_pointer += 1,
                    MESSAGE_OPEN_INVALID => {
                        self.finished = true;
                        return StepResult::Cancelled;
                    }
                    _ => {
                        return match self.pending_message.take() {
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
                        params: self.params.clone(),
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
                    if self.work_zone[0] == CHOICE_CANCELLED {
                        self.finished = true;
                        return StepResult::Cancelled;
                    }
                    self.exec_pointer += 1;
                }
                // XiEvent ReqSet/GetReqStatus family (research/XiEvents/OpCodes/
                // 0x0027.md–0x002A.md): actor-choreography sync points. This
                // dialog-only VM has no actors to wait on, so they complete
                // instantly; explicit arms because the fallback refuses sets_ret.
                OP_REQSET | OP_REQSET_CHECKED | OP_REQSET_PRIORITY | OP_REQWAIT => {
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                // XiEvent LOADEXTSCHEDULER (research/XiEvents/OpCodes/0x005B.md,
                // 0x0066.md): plays a motion between two actors, but always
                // takes the "actor not found" early exit since this dialog-only
                // VM models no actors.
                OP_LOADEXTSCHEDULER | OP_LOADEXTSCHEDULER2 => {
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                // XiEvent WAITSCHEDULOR/WAITMAPSCHEDULOR/WAITLOADSCHEDULER
                // (research/XiEvents/OpCodes/0x0053.md–0x0055.md): block until
                // two named actors' schedulers finish. Same early exit as the
                // loaders above — both actors have to resolve before retail
                // waits on anything, and this VM resolves none. Load-bearing:
                // the chocobo rental cutscene is one 0x53, and refusing it
                // auto-released the whole scene.
                OP_WAITSCHEDULOR | OP_WAITMAPSCHEDULOR | OP_WAITLOADSCHEDULER => {
                    self.exec_pointer += OPCODE_META[op as usize].size as usize;
                }
                OP_SCHEDULOR => {
                    self.cues.push(EventCue::ActorMotion {
                        actor1: ActorLookup(self.eventgetcode2(SCHEDULOR_ACTOR1_OFS)),
                        actor2: ActorLookup(self.eventgetcode2(SCHEDULOR_ACTOR2_OFS)),
                        key: self.fourcc_at(SCHEDULOR_KEY_OFS),
                    });
                    self.advance(op);
                }
                OP_LOADEVENTSCHEDULER2 => {
                    let file = dat_id_helper(self.getworkofs(LOADEVENTSCHEDULER2_FILE_OFS, 0));
                    self.cues.push(EventCue::Scheduler {
                        dat_id: SCHEDULER_DAT_ID_BASE.wrapping_add(file as u32),
                        actor1: ActorLookup(self.eventgetcode2(LOADEVENTSCHEDULER2_ACTOR1_OFS)),
                        actor2: ActorLookup(self.eventgetcode2(LOADEVENTSCHEDULER2_ACTOR2_OFS)),
                        tag: self.fourcc_at(LOADEVENTSCHEDULER2_TAG_OFS),
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
                    if self.work_zone[0] == CHOICE_CANCELLED {
                        self.work_zone[0] = CHOICE_CANCELLED_QUERYWAIT2;
                    }
                    self.exec_pointer += 1;
                }
                // The sub-byte-dispatched families. Their width is the case's,
                // not the table's widest, so an undocumented sub stops the VM
                // rather than falling back and landing mid-instruction.
                OP_LOOKSET | OP_EVENTPOSSET | OP_LOADROOM | OP_ITEMINFO | OP_ENTITYSPEED
                | OP_MOVE | OP_WINDOW | OP_MENU | OP_RENDERFLAG | OP_REQRESET | OP_STRINGOPS
                | OP_NAMESET | OP_SUBSCHED | OP_STATUSSET => {
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
                    self.exec_pointer += width as usize;
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

    /// Set `CliEventMessOpenFlag` and hold the message for the MESWAIT (0x23)
    /// that yields it — the shape every message opcode shares.
    fn open_message(&mut self, message_id: u32, speaker_index: Option<u16>) {
        self.message_open = MESSAGE_OPEN_AWAITING;
        self.pending_message = Some(EventMessage {
            message_id,
            speaker_index,
            params: self.params.clone(),
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
            return self.work_zone[(val - WORK_ZONE_BASE) as usize] as i32;
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
            self.work_zone[(val - WORK_ZONE_BASE) as usize] = value as u32;
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
    /// the available dialog-menu option flags. Per XiEvents OpCodes/0x0040.md,
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
    fn op_if(&mut self) {
        let kind = self.byte_at(5) & 0x0F;
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
        e.work_zone[0] = 11;
        e.work_zone[1] = 22;

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
    fn loadextscheduler_family_skips_by_size_and_continues() {
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
    fn a_script_that_loops_forever_stops_instead_of_hanging() {
        // GOTO 0: the tightest loop the bytecode can express. GOTO is
        // implemented, so this must report as a spin, not as work to do.
        let mut e = vm(vec![OP_GOTO, 0x00, 0x00], vec![]);
        assert_eq!(e.step(), StepResult::Spun(OP_GOTO));
        // And it stays stopped rather than spinning again on the next tick.
        assert_eq!(e.step(), StepResult::Done);
    }

    #[test]
    fn scheduler_wait_family_skips_by_size_and_continues() {
        // Sizes are load-bearing: these opcodes carry actor references the VM
        // steps over blind, so a wrong width lands mid-instruction.
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
        // Still parked on MESWAIT until dismissed.
        assert_eq!(e.step(), StepResult::AwaitMessageAck);
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
        // [5]=kind 1, [6..8]=target=9, [8]=0xFF(skip), [9]=END.
        let data = vec![
            OP_IF, 0x00, 0x80, 0x00, 0x80, 0x01, 0x09, 0x00, 0xFF, OP_END,
        ];
        let mut e = vm(data, vec![42]);
        assert_eq!(e.step(), StepResult::Done);
        assert_eq!(e.exec_pointer(), 9);
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
        // 0x42 (size 1, no jump/ret) is skipped; reaches END.
        let mut e = vm(vec![0x42, 0x42, OP_END], vec![]);
        assert_eq!(e.step(), StepResult::Done);
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
        // XiEvents OpCodes/0x0040.md, "Call 1": v1=0, v2=0x0F, src(5)=0,
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
            0x00, // 8..15: if work_zone[0]==ref2 -> 19
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
        FourCc, SCHEDULER_DURATION_FROM_DAT, SCHEDULER_FADE_DAT_ID, SCHEDULER_TAG_FADE_IN,
        SCHEDULER_TAG_FADE_OUT,
    };

    /// Run one choreography opcode (padded to its documented width) to END and
    /// return the cues it emitted.
    fn cues_of(op: u8, operands: &[u8], references: Vec<u32>) -> Vec<EventCue> {
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
        assert_eq!(e.step(), StepResult::Done, "op 0x{op:02X} must run to END");
        assert_eq!(e.exec_pointer(), width, "op 0x{op:02X} advanced wrong size");
        e.take_cues()
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
}
