use ffxi_dat::event_dat::EventBlock;

use crate::opcode_meta::OPCODE_META;

/// A message the VM asked to display: dialog string `message_id` from the zone
/// dialog DAT ([`ffxi_dat::dmsg::StringDat`]), spoken by the entity at
/// `speaker_index` (the event's `EntityTargetIndex[1]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventMessage {
    pub message_id: u32,
    pub speaker_index: u16,
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
    /// A message was shown (0x1D) and the VM is blocked on MESWAIT (0x23). The
    /// host displays it, then calls [`EventVm::dismiss_message`] + [`EventVm::step`].
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
    /// An opcode the VM does not implement and cannot safely skip (a jump or a
    /// yielding opcode); execution stops to avoid desyncing `ExecPointer`.
    Unimplemented(u8),
}

pub(crate) const OP_END: u8 = 0x00;
const OP_GOTO: u8 = 0x01;
const OP_IF: u8 = 0x02;
const OP_GET_STORE: u8 = 0x03;
const OP_WAIT: u8 = 0x1C;
const OP_JUMP: u8 = 0x1A;
const OP_RETURN: u8 = 0x1B;
pub(crate) const OP_MESSAGE: u8 = 0x1D;
const OP_EXECEND: u8 = 0x21;
pub(crate) const OP_MESWAIT: u8 = 0x23;
pub(crate) const OP_QUERY: u8 = 0x24;
pub(crate) const OP_QUERYWAIT: u8 = 0x25;
const OP_REQSET: u8 = 0x27;
const OP_REQSET_CHECKED: u8 = 0x28;
const OP_REQSET_PRIORITY: u8 = 0x29;
const OP_REQWAIT: u8 = 0x2A;
const OP_SETBITWORK: u8 = 0x40;
const OP_GETBITWORK: u8 = 0x41;
const OP_SENDTAG: u8 = 0x43;
const OP_SLEEP: u8 = 0x6F;
const OP_TURNWAIT: u8 = 0x70;

const MESSAGE_OPEN_NONE: u8 = 0;
const MESSAGE_OPEN_AWAITING: u8 = 1;
// CliEventMessOpenFlag = 2 is the invalid-open state MESWAIT force-cancels on
// (XiEvents OpCodes/0x0023.md).
const MESSAGE_OPEN_INVALID: u8 = 2;

const WORK_LOCAL_LEN: usize = 80;
const WORK_ZONE_LEN: usize = 96;
const WORK_ZONE_BASE: u32 = 4096;
const JUMP_STACK_LEN: usize = 8;
// References-table index marker; low bits index it (XiEvents Event VM Functions.md).
const REFERENCE_FLAG: u32 = 0x8000;
const REFERENCE_INDEX_MASK: u32 = 0x7FFF;
/// QUERYWAIT stores this in `Work_Zone[0]` when the player cancels the menu.
const CHOICE_CANCELLED: u32 = 254;

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
}

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
            finished: false,
            ran_past_end: false,
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

    /// Run opcodes until the VM yields (one `EventIdle` tick).
    pub fn step(&mut self) -> StepResult {
        if self.finished {
            return StepResult::Done;
        }
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
                    self.setworkofs(1, val);
                    self.exec_pointer += 5;
                }
                OP_SETBITWORK => {
                    self.op_bitwork(true);
                    self.exec_pointer += 9;
                }
                OP_GETBITWORK => {
                    self.op_bitwork(false);
                    self.exec_pointer += 9;
                }
                // 0x6F sleeps until ReqStack WaitTime expires, 0x70 yields while
                // the event entity is mid-turn; both then ExecPointer++. We model
                // no frame clock or entity render state, so they reduce to an
                // advance (XiEvents OpCodes/0x006F.md, 0x0070.md).
                OP_SLEEP | OP_TURNWAIT => self.exec_pointer += 1,
                // 0x1C is a timed wait (reads its duration, ticks it down each
                // frame, then advances +3) — also a no-frame-clock advance.
                OP_WAIT => self.exec_pointer += 3,
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
                    let message_id = self.getworkofs(1, 0) as u32;
                    self.message_open = MESSAGE_OPEN_AWAITING;
                    self.pending_message = Some(EventMessage {
                        message_id,
                        speaker_index: self.speaker_index,
                        params: self.params.clone(),
                    });
                    self.exec_pointer += 3;
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
                _ => {
                    let meta = OPCODE_META.get(op as usize).copied();
                    match meta {
                        Some(m) if m.valid && !m.jumps && !m.sets_ret && m.size > 0 => {
                            self.exec_pointer += m.size as usize;
                        }
                        _ => return StepResult::Unimplemented(op),
                    }
                }
            }
        }
    }

    /// `XiEvent::eventgetcode`: little-endian u16 at `ExecPointer + index`.
    /// Reads past the end of the bytecode yield 0 (retail reads unchecked
    /// memory; 0 is our deterministic stand-in) — they are counted in
    /// [`Self::oob_reads`] and the first one is logged (kuluu-zkuf).
    fn eventgetcode(&self, index: usize) -> u16 {
        let at = self.exec_pointer + index;
        if at + 1 >= self.event_data.len() {
            let seen = self.oob_reads.get();
            self.oob_reads.set(seen.saturating_add(1));
            if seen == 0 {
                tracing::debug!(
                    at,
                    bytecode_len = self.event_data.len(),
                    "eventgetcode read past end of bytecode (yields 0; \
                     further out-of-bounds reads counted, not logged)"
                );
            }
        }
        let lo = self.event_data.get(at).copied().unwrap_or(0);
        let hi = self.event_data.get(at + 1).copied().unwrap_or(0);
        u16::from_le_bytes([lo, hi])
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
    fn setworkofs(&mut self, index: usize, value: i32) {
        let val = self.eventgetcode(index) as u32;
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
            self.setworkofs(5, v3 | (mask & v4.wrapping_shl(shift)));
        } else {
            let v3 = self.getworkofs(5, 0);
            self.setworkofs(7, (mask & v3).wrapping_shr(shift));
        }
    }

    /// `XiEvent::CodeIF` (0x0002): conditional branch with 11 comparison kinds.
    fn op_if(&mut self) {
        let kind = self
            .event_data
            .get(self.exec_pointer + 5)
            .copied()
            .unwrap_or(0)
            & 0x0F;
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
    fn message_then_meswait_yields_then_resumes() {
        // 0x1D msg (ref index 0x8000 -> references[0]=900), then 0x23 MESWAIT, END.
        let mut e = vm(vec![OP_MESSAGE, 0x00, 0x80, OP_MESWAIT, OP_END], vec![900]);
        assert_eq!(
            e.step(),
            StepResult::AwaitMessage(EventMessage {
                message_id: 900,
                speaker_index: 5,
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
                speaker_index: 5,
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
                speaker_index: 5,
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
        // 0x3E is a jumping opcode we don't implement; it must not be skipped by
        // size (that would desync ExecPointer), so the VM stops.
        let mut e = vm(vec![0x3E, 0, 0, 0, 0, 0, 0], vec![]);
        assert_eq!(e.step(), StepResult::Unimplemented(0x3E));
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
                speaker_index: 5,
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
        let data = vec![OP_SLEEP, OP_TURNWAIT, OP_WAIT, 0x00, 0x00, OP_END];
        let mut e = vm(data, vec![]);
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
                speaker_index: 5,
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
}
