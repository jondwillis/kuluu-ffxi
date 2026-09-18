//! Session-side bridge from the event-trigger packets (0x32) to the event VM.
//!
//! Holds the per-zone event + dialog DATs and the active [`DialogRunner`] across
//! player interactions, turning VM yields into real [`DialogState`]s. When no
//! event DAT can drive a trigger, [`DialogSession::begin`] returns
//! [`Begin::Undriveable`] and the caller auto-releases the event (EVENT_END)
//! rather than pin the character InEvent behind an empty dialog.

use std::sync::Arc;

use ffxi_dat::dmsg::{
    plain_marker, StringDat, MARKER_CHOCOBO_NAME, MARKER_ITEM, MARKER_KEY_ITEM, MARKER_NUM,
    MARKER_PLAYER_NAME, MARKER_SPEAKER_NAME,
};
use ffxi_dat::event_dat::{EventBlockSource, EventDat};
use ffxi_dat::kind::ChunkKind;
use ffxi_dat::scheduler::Scheduler;
use ffxi_dat::DatRoot;
use ffxi_event::{ActorLookup, DialogRunner, DialogStep, EventCue, FourCc, PendingTag};
use tokio::sync::broadcast;

use crate::state::{AgentEvent, CutsceneActor, CutsceneCue, DialogState};

struct ActiveEvent {
    unique_no: u32,
    act_index: u16,
    /// The event id the VM runs — `EventPara` from the trigger packet, echoed in
    /// the 0x05B EVENT_END `EventPara` field the server validates.
    event_id: u16,
    /// Opaque id for the agent event stream (`unique_no << 16 | event_id`).
    agent_event_id: u32,
    npc_name: Option<String>,
}

/// Outcome of advancing an in-progress event after a player response.
pub enum Advance {
    /// Show the next frame.
    Frame(DialogState),
    /// The event is over — the caller sends EVENT_END with `end_para` as the
    /// 0x05B `EndPara` (the VM's `Work_Zone[1]`, or a cancel sentinel).
    Ended {
        end_para: u32,
        final_position: Option<ffxi_event::vm::scene::EventPosition>,
    },
    /// The scene is holding on a timed wait: no frame to show, and the event
    /// stays open. The caller must not send EVENT_END on this.
    Waiting,
    /// The VM holds a pending tag awaiting its s2c ack: the caller sends the
    /// c2s half now and holds until EVENTUCOFF EventRecvPending arrives; no
    /// frame to show, and the event stays open (no EVENT_END on this either).
    AwaitServerAck(PendingTag),
}

/// Outcome of starting a VM-driven event.
/// Why an event could not be driven. Collapsing these into one message hid
/// which half of the pipeline failed: a missing string DAT and an event id no
/// block in the zone authors are different bugs with the same symptom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndriveableReason {
    /// No dialog (dmsg) DAT for the zone — every event in it is undriveable.
    NoStrings,
    /// No event DAT for the zone.
    NoEventDat,
    /// No block in the zone authors this event id — not the entity's own, not
    /// the zone master, and not a sole owner elsewhere (see
    /// [`EventDat::block_for_event`]).
    ///
    /// [`EventDat::block_for_event`]: ffxi_dat::event_dat::EventDat::block_for_event
    NoEventEntry,
    /// The VM stopped on an opcode it cannot advance past.
    StoppedOnOpcode,
}

impl UndriveableReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoStrings => "no string DAT for zone",
            Self::NoEventDat => "no event DAT for zone",
            Self::NoEventEntry => "no block in this zone authors that event id",
            Self::StoppedOnOpcode => "unimplemented opcode",
        }
    }
}

pub enum Begin {
    /// Show the first frame and wait for the player.
    Frame(DialogState),
    /// The VM ran the whole event without producing a dialog frame
    /// (choreography-only or bookkeeping script) — the caller sends EVENT_END
    /// with `end_para`, same as [`Advance::Ended`].
    Ended { end_para: u32 },
    /// The VM can't drive the event. `stopped_op` is set only when the bytecode
    /// itself hit an opcode we cannot advance past; the other reasons never
    /// reach the VM at all.
    Undriveable {
        stopped_op: Option<u8>,
        reason: UndriveableReason,
    },
    /// The scene opened on a timed wait — a fade before the first line. The
    /// event stays open and [`DialogSession::tick`] carries it forward.
    Waiting,
    /// The VM opened holding a pending tag: the caller sends the c2s half now;
    /// the s2c ack drives [`DialogSession::ack_server`], which resumes with the
    /// first frame or end.
    AwaitServerAck(PendingTag),
}

/// One server-dispatched event trigger, normalised across 0x32/0x33/0x34.
pub struct EventTrigger {
    /// Zone whose event-bytecode DAT authors the script (`EventNum`).
    pub event_zone: u16,
    /// Zone whose dialog DAT holds the strings. Usually the same as
    /// `event_zone`, but 0x34 can redirect it (`EventNum2` = `eventInfo->
    /// textTable`, vendor/server/src/map/packets/s2c/0x034_eventnum.cpp GP_SERV_COMMAND_EVENTNUM::GP_SERV_COMMAND_EVENTNUM).
    pub text_zone: u16,
    pub unique_no: u32,
    pub act_index: u16,
    pub event_id: u16,
    /// `num[8]`, the numerics behind the `{Num:N}` markers. Empty on 0x32,
    /// which carries none.
    pub params: Vec<i32>,
    pub npc_name: Option<String>,
}

pub struct DialogSession {
    dat_root: Option<Arc<DatRoot>>,
    /// Logged-in character name, substituted for the `{PlayerName}` dialog marker.
    player_name: String,
    loaded_event_zone: Option<u16>,
    loaded_string_zone: Option<u16>,
    event_dat: Option<Arc<EventDat>>,
    player_position: Option<ffxi_event::vm::scene::EventPosition>,
    scene_actions: Vec<ffxi_event::vm::scene::SceneAction>,
    strings: Option<StringDat>,
    runner: Option<DialogRunner>,
    active: Option<ActiveEvent>,
    /// Cues drained from the VM after each step, held until the caller takes
    /// them: a step that ends the event still emits them (the chocobo rental's
    /// fade-in lands in the step that ends the script).
    cues: Vec<ResolvedCue>,
    /// Per-zone fishing-era reconciliation state, built lazily on the first
    /// TALKNUM-family message of the zone.
    fishing: std::collections::HashMap<u16, FishingEra>,
    /// Authored routine lengths the WAIT* holds arm from, cached per (dat id,
    /// tag) with misses included so a re-issued routine does not re-read its
    /// file.
    routine_lengths: std::collections::HashMap<(u32, FourCc), Option<f32>>,
    /// Last known position of every entity the server has placed since the
    /// zone-in, in event coordinates: the source for MOVE hold lengths while a
    /// scene walks its actors.
    entity_positions: std::collections::HashMap<u32, ffxi_event::vm::scene::EventPosition>,
    /// The retail entity Type byte (ent+0xEE) of every 0x0E'd entity, keyed by
    /// the entity's server id and target index: the input the VM's 0x5B/0x66
    /// load gate reads. Fed from
    /// [`crate::session::event_transport::receive`]; an absent entry is Type 0.
    entity_types: std::collections::HashMap<u32, u8>,
    /// Motion holds awaiting the renderer's finish report, keyed by the wire
    /// actor the cue named plus its key: the value is the VM's own unresolved
    /// lookup (for the release), the count of outstanding issues for the pair
    /// (a repeat issue re-arms before the first finishes), the arm time the
    /// deadline sweep measures from, and the deadline itself: the DAT-authored
    /// routine length when the session can read it, so a renderer-less session
    /// times out on the authored length itself, else [`PENDING_MOTION_HOLD_MAX`].
    pending_motion_holds: std::collections::HashMap<
        (CutsceneActor, FourCc),
        (ActorLookup, u32, std::time::Instant, std::time::Duration),
    >,
    /// Whether a frame was displayed before the last step, so
    /// [`take_message_closed`](Self::take_message_closed) fires exactly once per
    /// up→down transition instead of on every tick parked after it.
    message_was_up: bool,
}

impl DialogSession {
    pub fn new(dat_root: Option<Arc<DatRoot>>, player_name: String) -> Self {
        Self {
            dat_root,
            player_name,
            loaded_event_zone: None,
            loaded_string_zone: None,
            event_dat: None,
            player_position: None,
            scene_actions: Vec::new(),
            strings: None,
            runner: None,
            active: None,
            cues: Vec::new(),
            fishing: std::collections::HashMap::new(),
            routine_lengths: std::collections::HashMap::new(),
            entity_positions: std::collections::HashMap::new(),
            entity_types: std::collections::HashMap::new(),
            pending_motion_holds: std::collections::HashMap::new(),
            message_was_up: false,
        }
    }

    /// The staging cues the VM emitted since the last drain, in execution
    /// order, with their actors resolved. Empty after one call. Drain after
    /// every [`begin`](Self::begin) / [`advance`](Self::advance) /
    /// [`cancel`](Self::cancel), including the ones that ended the event.
    pub fn take_cues(&mut self) -> Vec<ResolvedCue> {
        std::mem::take(&mut self.cues)
    }

    /// `(unique_no, act_index, event_id)` of the active VM-driven event, for the
    /// EVENT_END reply. `None` when no VM event is running (legacy path).
    pub fn active_end(&self) -> Option<(u32, u16, u16)> {
        self.active
            .as_ref()
            .map(|a| (a.unique_no, a.act_index, a.event_id))
    }

    fn ensure_event_dat(&mut self, zone: u16) {
        if self.loaded_event_zone == Some(zone) {
            return;
        }
        self.loaded_event_zone = Some(zone);
        self.event_dat = load_event_dat(self.dat_root.as_deref(), zone).map(Arc::new);
    }

    fn ensure_strings(&mut self, zone: u16) {
        if self.loaded_string_zone == Some(zone) {
            return;
        }
        self.loaded_string_zone = Some(zone);
        self.strings = load_strings(self.dat_root.as_deref(), zone);
    }

    /// Begin a VM-driven event for a server trigger.
    pub fn begin(&mut self, trigger: EventTrigger) -> Begin {
        self.clear();
        let EventTrigger {
            event_zone,
            text_zone,
            unique_no,
            act_index,
            event_id,
            params,
            npc_name,
        } = trigger;
        self.ensure_event_dat(event_zone);
        self.ensure_strings(text_zone);
        let undriveable = |reason| Begin::Undriveable {
            stopped_op: None,
            reason,
        };
        let Some(strings) = self.strings.as_ref() else {
            return undriveable(UndriveableReason::NoStrings);
        };
        let Some(dat) = self.event_dat.as_ref() else {
            return undriveable(UndriveableReason::NoEventDat);
        };
        let Some((block, source)) = ffxi_event::EventVm::driving_block(dat, unique_no, event_id)
        else {
            return undriveable(UndriveableReason::NoEventEntry);
        };
        if source != EventBlockSource::OwnBlock {
            tracing::info!(
                zone = event_zone,
                unique_no = format!("0x{unique_no:08X}"),
                event_id,
                ?source,
                "event program resolved on another participating actor"
            );
        }
        let Some(mut runner) = DialogRunner::start(block, event_id, act_index, params) else {
            return undriveable(UndriveableReason::NoEventEntry);
        };
        runner.set_actor_types(&self.entity_types);
        if let Some(position) = self.player_position {
            runner.attach_scene(dat.clone(), block.actor, position);
            // Multi-entity events run every owner block in parallel from event
            // start (retail's per-entity event instances, research/XiEvents/
            // Event VM Functions.md InitEvent2/XiEventInit): spawn a child for
            // each owner block the master does not already drive. The children
            // keep the event alive until they all drain; their cues bubble up.
            for (owner, entry) in ffxi_event::EventVm::owner_blocks(dat, event_id) {
                if owner.actor != block.actor {
                    runner.spawn_owner(owner, entry);
                }
            }
        }
        let step = runner.advance(None, strings);
        self.scene_actions.extend(runner.take_scene_actions());
        let raw_cues = runner.take_cues();
        arm_motion_holds(
            &mut runner,
            &raw_cues,
            self.dat_root.as_deref(),
            &mut self.routine_lengths,
            unique_no,
            event_zone,
            &mut self.pending_motion_holds,
        );
        arm_move_holds(&mut runner, &raw_cues, &self.entity_positions, unique_no);
        self.cues.extend(
            raw_cues
                .into_iter()
                .map(|c| resolve_cue(c, unique_no, event_zone)),
        );
        let active = ActiveEvent {
            unique_no,
            act_index,
            event_id,
            agent_event_id: agent_event_id(unique_no, event_id),
            npc_name,
        };
        match step {
            DialogStep::Frame(frame) => {
                let dialog =
                    frame_to_dialog(&active, frame, &self.player_name, runner.cancel_armed());
                self.runner = Some(runner);
                self.active = Some(active);
                Begin::Frame(dialog)
            }
            DialogStep::Ended { end_para } => {
                self.finish();
                Begin::Ended { end_para }
            }
            DialogStep::Stopped(op) => {
                self.finish();
                Begin::Undriveable {
                    stopped_op: Some(op),
                    reason: UndriveableReason::StoppedOnOpcode,
                }
            }
            DialogStep::Waiting => {
                self.runner = Some(runner);
                self.active = Some(active);
                Begin::Waiting
            }
            DialogStep::AwaitServerAck(tag) => {
                self.runner = Some(runner);
                self.active = Some(active);
                Begin::AwaitServerAck(tag)
            }
        }
    }

    pub(crate) fn step(
        &mut self,
        drive: crate::session::event_transport::Drive,
        _permit: &crate::session::event_transport::DrivePermit,
    ) -> Advance {
        use crate::session::event_transport::Drive;
        match drive {
            Drive::Cancel => self.cancel(),
            Drive::Choice(choice) => self.advance(Some(choice)),
            Drive::Tick(seconds) => self.tick(seconds),
        }
    }

    /// Apply the player's response (dismiss, or `Some(index)` choice) and return
    /// the next frame or [`Advance::Ended`]. Call only while [`active_end`] is
    /// `Some`.
    ///
    /// [`active_end`]: Self::active_end
    fn advance(&mut self, choice: Option<u32>) -> Advance {
        self.drive(|runner, strings| runner.advance(choice, strings))
    }

    /// Cancel the in-progress event from any frame (the Esc path): the VM
    /// reports the frame's cancel result and ends with
    /// [`ffxi_event::EVENT_CANCELLED_END_PARA`].
    fn cancel(&mut self) -> Advance {
        self.scene_actions.clear();
        self.drive(|runner, strings| runner.cancel(strings))
    }

    /// Run the host clock into a scene holding on a timed wait; a no-op
    /// ([`Advance::Waiting`]) while a frame is displayed instead. Call only
    /// while [`active_end`] is `Some` — like [`advance`](Self::advance), a
    /// desynced call releases the event rather than wedging it open.
    ///
    /// [`active_end`]: Self::active_end
    fn tick(&mut self, dt_secs: f32) -> Advance {
        self.sweep_pending_motion_holds();
        self.drive(|runner, strings| runner.tick(dt_secs, strings))
    }

    /// The server acknowledged the pending tag: both c2s event-end process()
    /// functions push s2c EVENTUCOFF EventRecvPending right after handling it,
    /// and that is the release of the VM's case-1 hold. Call only while a tag
    /// is in flight ([`has_pending_tag`](Self::has_pending_tag)); like
    /// [`advance`](Self::advance), a desynced call releases the event rather
    /// than wedging it open.
    pub fn ack_server(&mut self) -> Advance {
        self.drive(|runner, strings| runner.ack_server(strings))
    }

    /// s2c PENDINGNUM's num[8] into the VM's Work_Zone from index 2, where the
    /// event system reads its loop conditions; lands before the next step even
    /// while a tag is held (research/XiPackets/world/server/0x005C). No-op when
    /// no VM event runs.
    pub fn apply_pending_num(&mut self, num: &[i32; 8]) {
        if let Some(runner) = self.runner.as_mut() {
            runner.apply_pending_num(num);
        }
    }

    /// s2c PENDINGSTR's four strings into the VM's event string table, where
    /// 0xB4 case 1 reads them; lands before the next step even while a tag is
    /// held (research/XiPackets/world/server/0x005D). No-op when no VM event
    /// runs.
    pub fn apply_pending_str(&mut self, strings: &[[u8; 16]; 4]) {
        if let Some(runner) = self.runner.as_mut() {
            runner.apply_pending_str(strings);
        }
    }

    /// True while any VM in the event (the master or an owner child) holds a
    /// pending tag awaiting its s2c ack — the tag is one global per event in
    /// retail, so a child's held tag gates the drain too. While true the
    /// owning event must not be drained by a Mode-0 EVENT_END: that would
    /// kill the server-side event mid-transaction and OnEventUpdate would find
    /// no currentEvent (vendor/server/src/map/packets/c2s/0x05b_eventend.cpp).
    pub fn has_pending_tag(&self) -> bool {
        self.runner
            .as_ref()
            .is_some_and(|r| r.pending_tag().is_some())
    }

    /// True exactly once per up→down transition of the displayed frame: call it
    /// after every step and emit [`AgentEvent::DialogDismissed`](crate::state::AgentEvent)
    /// when it fires. Retail clears CliEventMessOpenFlag on dismissal, so the
    /// box hides until the next message opcode reopens it; a plain
    /// `Advance::Waiting` cannot signal this because ticks parked while a frame
    /// is up report Waiting too.
    pub fn take_message_closed(&mut self) -> bool {
        let up = self.runner.as_ref().is_some_and(|r| r.message_awaiting());
        let closed = self.message_was_up && !up;
        self.message_was_up = up;
        closed
    }

    fn drive(&mut self, step: impl FnOnce(&mut DialogRunner, &StringDat) -> DialogStep) -> Advance {
        let types = self.entity_types.clone();
        let (Some(strings), Some(runner), Some(active)) = (
            self.strings.as_ref(),
            self.runner.as_mut(),
            self.active.as_ref(),
        ) else {
            self.finish();
            return Advance::Ended {
                end_para: 0,
                final_position: None,
            };
        };
        let event_entity = active.unique_no;
        runner.set_actor_types(&types);
        let outcome = step(runner, strings);
        let final_position = runner.controlled_position();
        self.scene_actions.extend(runner.take_scene_actions());
        let raw_cues = runner.take_cues();
        let zone = self.loaded_event_zone.unwrap_or(0);
        arm_motion_holds(
            runner,
            &raw_cues,
            self.dat_root.as_deref(),
            &mut self.routine_lengths,
            event_entity,
            zone,
            &mut self.pending_motion_holds,
        );
        arm_move_holds(runner, &raw_cues, &self.entity_positions, event_entity);
        let cues: Vec<ResolvedCue> = raw_cues
            .into_iter()
            .map(|c| resolve_cue(c, event_entity, zone))
            .collect();
        let advance = match outcome {
            DialogStep::Frame(frame) => Advance::Frame(frame_to_dialog(
                active,
                frame,
                &self.player_name,
                runner.cancel_armed(),
            )),
            DialogStep::Ended { end_para } => Advance::Ended {
                end_para,
                final_position,
            },
            DialogStep::Stopped(op) => {
                tracing::warn!(
                    op = format!("0x{op:02X}"),
                    "event VM stopped mid-dialog; releasing with end_para 0"
                );
                Advance::Ended {
                    end_para: 0,
                    final_position: None,
                }
            }
            DialogStep::Waiting => Advance::Waiting,
            DialogStep::AwaitServerAck(tag) => Advance::AwaitServerAck(tag),
        };
        self.cues.extend(cues);
        if matches!(advance, Advance::Ended { .. }) {
            self.finish();
        }
        advance
    }

    pub fn set_player_position(&mut self, position: ffxi_event::vm::scene::EventPosition) {
        self.player_position = Some(position);
    }

    /// Remember where the server placed an entity, in event coordinates:
    /// feeds [`arm_move_holds`] when a scene walks that actor.
    pub fn note_entity_position(
        &mut self,
        id: u32,
        position: ffxi_event::vm::scene::EventPosition,
    ) {
        self.entity_positions.insert(id, position);
    }

    /// Remember the retail entity Type byte a 0x0E assigned to an entity,
    /// under both its server id and target index: the VM's 0x5B/0x66 gate
    /// resolves a named actor to one of those keys (retail's GetActorIndex,
    /// research/XiEvents/Event VM Functions.md).
    pub fn note_entity_type(&mut self, unique_no: u32, act_index: u16, type_: u8) {
        self.entity_types.insert(unique_no, type_);
        self.entity_types.insert(act_index as u32, type_);
    }

    pub fn controls_player_position(&self) -> bool {
        self.runner
            .as_ref()
            .is_some_and(|r| r.controls_player_position())
    }

    pub(crate) fn drain_scene_actions(
        &mut self,
        _permit: &crate::session::event_transport::DrivePermit,
    ) -> Vec<ffxi_event::vm::scene::SceneAction> {
        self.take_scene_actions()
    }

    fn take_scene_actions(&mut self) -> Vec<ffxi_event::vm::scene::SceneAction> {
        std::mem::take(&mut self.scene_actions)
    }

    pub fn acknowledge_position(&mut self, position: ffxi_event::vm::scene::EventPosition) {
        if let Some(runner) = &mut self.runner {
            runner.acknowledge_position(position);
        }
    }

    pub fn reject_position(&mut self) {
        if let Some(runner) = &mut self.runner {
            runner.reject_position();
        }
    }

    pub fn acknowledge_event(&mut self) {
        if let Some(runner) = &mut self.runner {
            runner.acknowledge_event();
        }
    }

    pub fn clear(&mut self) {
        self.scene_actions.clear();
        self.finish();
    }

    fn finish(&mut self) {
        self.runner = None;
        self.active = None;
        self.message_was_up = false;
        self.pending_motion_holds.clear();
    }

    /// The renderer finished (or could not start) the motion routine this wire
    /// `(actor, key)` named: decrement the pending hold's issue count and
    /// release the VM's hold when the last one lands. No-op when nothing is
    /// pending for the pair (stray report, event already ended).
    pub fn motion_done(&mut self, actor: CutsceneActor, key: FourCc) {
        let Some(entry) = self.pending_motion_holds.get_mut(&(actor, key)) else {
            return;
        };
        entry.1 = entry.1.saturating_sub(1);
        if entry.1 > 0 {
            return;
        }
        let (lookup, _, _, _) = self
            .pending_motion_holds
            .remove(&(actor, key))
            .expect("entry held above");
        if let Some(runner) = self.runner.as_mut() {
            runner.release_action_hold(lookup, key);
        }
    }

    /// Last-resort release for a motion hold that waits out its deadline:
    /// a stopped routine, a despawned entity, or a session with no renderer at
    /// all. Each hold ages out on its own deadline (the DAT-authored routine
    /// length, or [`PENDING_MOTION_HOLD_MAX`] when the session cannot read the
    /// DAT); the sweep is the degradation path, not the clock.
    fn sweep_pending_motion_holds(&mut self) {
        let now = std::time::Instant::now();
        let stale: Vec<(CutsceneActor, FourCc)> = self
            .pending_motion_holds
            .iter()
            .filter(|(_, (_, _, armed, deadline))| now.duration_since(*armed) > *deadline)
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            let Some((lookup, _, armed, _)) = self.pending_motion_holds.remove(&key) else {
                continue;
            };
            tracing::warn!(
                target: "kuluu_session::event_dialog",
                age_secs = now.duration_since(armed).as_secs_f32(),
                "motion hold released on its deadline without a renderer finish report"
            );
            if let Some(runner) = self.runner.as_mut() {
                runner.release_action_hold(lookup, key.1);
            }
        }
    }

    /// Entry `index` of `zone`'s dialog DAT, loading it if needed. `None` when
    /// the zone has no available string DAT (missing FFXI_DAT_PATH, unmapped
    /// zone) or the index is out of range.
    pub fn zone_text(&mut self, zone: u16, index: usize) -> Option<String> {
        self.ensure_strings(zone);
        self.strings.as_ref()?.text(index)
    }

    /// [`Self::zone_text`] restricted to printable lines. An entry carrying a
    /// Selection control code is a menu — prompt plus options — which retail
    /// drives through the event VM; a menu entry is never a chat line, and
    /// returning `None` keeps the caller's placeholder.
    pub fn zone_chat_text(&mut self, zone: u16, index: usize) -> Option<String> {
        self.ensure_strings(zone);
        let dat = self.strings.as_ref()?;
        if dat.menu(index).is_some() {
            tracing::warn!(
                zone,
                index,
                "zone message names a menu entry, not a chat line; keeping the placeholder"
            );
            return None;
        }
        dat.text(index)
    }

    /// Resolve a TALKNUM-family message as a fishing line, reconciling the
    /// client-era skew between the server's text ids and this install's
    /// dialog DAT. The wire id is `server_base + offset`; the DAT entry lives
    /// at `install_base + offset`, and the two bases differ whenever the
    /// server and the install were built for different client eras: against
    /// the vendored LSB pin's fishing base, KNOWN_CLIENTS horizonxi-2023 sits
    /// 8-10 entries below and retail-2026-09 sits 4 above (LSB origin/base at
    /// 30260904_1 matches retail-2026-09 exactly). Without reconciliation
    /// every fishing line renders as whatever entry the skew lands on —
    /// another line entirely, or the menu-guard placeholder.
    pub fn fishing_chat(&mut self, zone: u16, mes_num: u16, opcode: u16) -> FishingChat {
        let Some(pin_base) = ffxi_proto::fishing_messages::zone_offset(zone) else {
            return FishingChat::NotFishing;
        };
        self.ensure_strings(zone);
        let era = self.fishing.entry(zone).or_default();
        let install_base = match era.install_base {
            Some(found) => found,
            None => {
                let found = self.strings.as_ref().and_then(find_fishing_block);
                if found.is_none() {
                    tracing::debug!(
                        zone,
                        "no fishing block landmarks in the zone dialog DAT; era \
                         reconciliation disabled for this zone"
                    );
                }
                era.install_base = Some(found);
                found
            }
        };
        let Some(install_base) = install_base else {
            return FishingChat::NotFishing;
        };

        let era = self.fishing.get_mut(&zone).expect("entry inserted above");
        let server = std::mem::take(&mut era.server);
        let dat = self.strings.as_ref().expect("strings ensured above");
        let printable = |offset: u8| -> Option<String> {
            let index = install_base as usize + offset as usize;
            if dat.menu(index).is_some() {
                return None;
            }
            dat.text(index)
        };
        let (chat, server) =
            resolve_fishing(&printable, pin_base, install_base, mes_num, opcode, server);
        self.fishing
            .get_mut(&zone)
            .expect("entry inserted above")
            .server = server;
        chat
    }
}

/// Opaque id for the agent event stream, joining the triggering entity to the
/// event it runs. Emitted by [`DialogSession::begin`] as
/// [`DialogState::event_id`] and by the cutscene channel as
/// [`AgentEvent::CutsceneStarted::event_id`]; the two must agree.
pub fn agent_event_id(unique_no: u32, event_id: u16) -> u32 {
    ((unique_no as u64) << 16 | event_id as u64) as u32
}

/// A drained [`EventCue`] with its actors resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedCue {
    /// Crosses the wire boundary as a [`CutsceneCue`].
    Scene(CutsceneCue),
    /// 0x5D rides the existing [`AgentEvent::MusicVolumeChanged`] instead of
    /// the cue stream.
    MusicVolume { volume: u8, fade_frames: u16 },
    /// 0xC8/0x8B/0x8A ride their own map AgentEvents: the Map screen is client
    /// UI state, not scene state.
    Map(MapOp),
}

/// One event-script ask on the player's Map screen (research/XiEvents/OpCodes/
/// 0x00C8.md, 0x008B.md, 0x008A.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapOp {
    /// Open the Map on zone `map_id`; `tutorial` is retail's help-flag operand.
    Open { map_id: u16, tutorial: bool },
    /// Place a named marker at milli-unit coordinates on zone `map_id`'s map.
    Marker {
        map_id: u16,
        x_milli: i32,
        y_milli: i32,
        label: String,
    },
    /// Close the Map screen.
    Close,
}

/// The cue's motion resource across the wire boundary: the VM's type is
/// isomorphic to the wire's, so this is a field-for-field copy.
fn snapshot_motion(motion: ffxi_event::ExtSchedulerMotion) -> kuluu_snapshot::ExtSchedulerMotion {
    match motion {
        ffxi_event::ExtSchedulerMotion::Event(id) => kuluu_snapshot::ExtSchedulerMotion::Event(id),
        ffxi_event::ExtSchedulerMotion::Tpc(pkgs) => kuluu_snapshot::ExtSchedulerMotion::Tpc {
            a: pkgs.a,
            b_set: pkgs.b_set,
            b_clear: pkgs.b_clear,
        },
    }
}

/// Resolve one VM cue against `event_entity`, the server id of the entity the
/// running event belongs to. `zone` is the event's zone, carried on the 0x2D
/// ZoneScheduler cue so the host resolves its key out of the zone's own model
/// DAT (the VM does not know it).
pub fn resolve_cue(cue: EventCue, event_entity: u32, zone: u16) -> ResolvedCue {
    let actor = |lookup| resolve_actor(lookup, event_entity);
    ResolvedCue::Scene(match cue {
        EventCue::ActorMotion {
            actor1,
            actor2,
            key,
        } => CutsceneCue::ActorMotion {
            actor: actor(actor1),
            partner: actor(actor2),
            key,
        },
        EventCue::Scheduler {
            dat_id,
            actor1,
            actor2,
            tag,
            duration,
        } => CutsceneCue::Scheduler {
            dat_id,
            actor: actor(actor1),
            partner: actor(actor2),
            tag,
            duration,
        },
        EventCue::ExtScheduler {
            motion,
            actor1,
            actor2,
            key,
        } => CutsceneCue::ExtScheduler {
            motion: motion.map(snapshot_motion),
            actor: actor(actor1),
            partner: actor(actor2),
            key,
        },
        EventCue::ZoneScheduler {
            key,
            actor1,
            actor2,
        } => CutsceneCue::ZoneScheduler {
            key,
            actor: actor(actor1),
            partner: actor(actor2),
            zone_id: zone,
        },
        EventCue::ActorHide { target, hide } => CutsceneCue::ActorHide {
            target: actor(target),
            hide,
        },
        EventCue::CameraLock { lock } => CutsceneCue::CameraLock { lock },
        EventCue::HudHide { hide } => CutsceneCue::HudHide { hide },
        EventCue::ClockHold { stop, hour } => CutsceneCue::ClockHold { stop, hour },
        EventCue::Mount {
            target,
            status_event,
            mount_id,
        } => CutsceneCue::Mount {
            target: actor(target),
            status_event,
            mount_id,
        },
        EventCue::ActorMove {
            actor: target,
            goal,
            speed,
        } => CutsceneCue::ActorMove {
            actor: actor(target),
            x: goal.x,
            y: goal.y,
            z: goal.z,
            heading: goal.heading,
            speed,
        },
        EventCue::ActorPlace {
            actor: target,
            position,
        } => CutsceneCue::ActorPlace {
            actor: actor(target),
            x: position.x,
            y: position.y,
            z: position.z,
            heading: position.heading,
        },
        EventCue::ActorFace {
            actor: target,
            heading,
        } => CutsceneCue::ActorFace {
            actor: actor(target),
            heading,
        },
        EventCue::ActorLookAt {
            actor: from,
            target: toward,
        } => CutsceneCue::ActorLookAt {
            actor: actor(from),
            target: actor(toward),
        },
        EventCue::ActorStopAction { actor: target, key } => CutsceneCue::ActorStopAction {
            actor: actor(target),
            key,
        },
        EventCue::MusicVolume {
            volume,
            fade_frames,
        } => {
            return ResolvedCue::MusicVolume {
                volume,
                fade_frames,
            }
        }
        EventCue::MapOpen { map_id, tutorial } => {
            return ResolvedCue::Map(MapOp::Open {
                map_id: map_id as u16,
                tutorial,
            })
        }
        EventCue::MapMarker {
            map_id,
            x_milli,
            y_milli,
            name,
        } => {
            // The 16-byte field is NUL-padded; retail's underscore rewrite
            // already happened in the VM.
            let label = String::from_utf8_lossy(&name)
                .trim_end_matches('\0')
                .trim()
                .to_string();
            return ResolvedCue::Map(MapOp::Marker {
                map_id: map_id as u16,
                x_milli,
                y_milli,
                label,
            });
        }
        EventCue::MapClose => return ResolvedCue::Map(MapOp::Close),
        EventCue::EntityName {
            actor: target,
            name,
        } => CutsceneCue::EntityName {
            actor: actor(target),
            name,
        },
    })
}

/// The event-entity selector and the default handler's fallback both mean "the
/// entity this event belongs to"; only a literal server id names another.
fn resolve_actor(lookup: ActorLookup, event_entity: u32) -> CutsceneActor {
    if lookup.is_local_player() {
        return CutsceneActor::LocalPlayer;
    }
    CutsceneActor::Entity {
        server_id: lookup.server_id().unwrap_or(event_entity),
    }
}

/// Why an event session is closing. Every variant releases the scope
/// identically; the enum exists so each call site names its exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSessionExit {
    /// The script reached END/EXECEND, or the 0x05B EVENT_END went out.
    ScriptEnded,
    /// The player escaped the frame, or the server cancelled the event.
    Cancelled,
    /// The pinned-event watchdog grace expired, or the player walked away.
    WatchdogReleased,
    ZoneChanged,
    Disconnected,
}

/// The event session's ownership of the client state its cues change.
///
/// 0x46 case 1 takes camera control and 1310 of 5269 retail event bodies never
/// issue the matching case 0 — an unpaired lock is the norm, and retail's de
/// facto release is the zone change. So the lock is scoped to the session and
/// dropped at every [`EventSessionExit`], never latched by the bytecode alone:
/// a permanently locked camera after a cutscene is worse than no lock at all.
#[derive(Debug, Default)]
pub struct CutsceneScope {
    open: bool,
    camera_locked: bool,
    published: bool,
}

impl CutsceneScope {
    /// Open the session. Idempotent while one is already open.
    pub fn start(&mut self, event_id: u32, event_tx: &broadcast::Sender<AgentEvent>) {
        if self.open {
            return;
        }
        self.open = true;
        let _ = event_tx.send(AgentEvent::CutsceneStarted { event_id });
    }

    /// Publish one resolved cue, recording the scope-owned state it takes.
    pub fn push(&mut self, cue: ResolvedCue, event_tx: &broadcast::Sender<AgentEvent>) {
        self.published = true;
        match cue {
            ResolvedCue::Scene(cue) => {
                if let CutsceneCue::CameraLock { lock } = cue {
                    self.camera_locked = lock;
                }
                let _ = event_tx.send(AgentEvent::CutsceneCue { cue });
            }
            ResolvedCue::MusicVolume {
                volume,
                fade_frames,
            } => {
                tracing::debug!(volume, fade_frames, "event script set music volume (0x5D)");
                for slot in 0..crate::state::MUSIC_SLOT_COUNT {
                    let _ = event_tx.send(AgentEvent::MusicVolumeChanged { slot, volume });
                }
            }
            ResolvedCue::Map(op) => {
                let ev = match op {
                    MapOp::Open { map_id, tutorial } => AgentEvent::MapOpen { map_id, tutorial },
                    MapOp::Marker {
                        map_id,
                        x_milli,
                        y_milli,
                        label,
                    } => AgentEvent::MapMarkerPlaced {
                        map_id,
                        x_milli,
                        y_milli,
                        label,
                    },
                    MapOp::Close => AgentEvent::MapClosed,
                };
                let _ = event_tx.send(ev);
            }
        }
    }

    /// Close the session, releasing everything it still owns. Idempotent, and
    /// safe to call on a path that had no event running.
    pub fn end(&mut self, exit: EventSessionExit, event_tx: &broadcast::Sender<AgentEvent>) {
        if self.camera_locked {
            self.camera_locked = false;
            let _ = event_tx.send(AgentEvent::CutsceneCue {
                cue: CutsceneCue::CameraLock { lock: false },
            });
        }
        // A cue can still be published after the scope closed — a server
        // CancelEvent shuts the scope while the runner lives on, and the next
        // drive pushes into it. Anything that reached the renderer has to be
        // released, or a fade latches the screen black with no driver left.
        if !self.open && !self.published {
            return;
        }
        self.open = false;
        self.published = false;
        tracing::debug!(?exit, "event session closed");
        let _ = event_tx.send(AgentEvent::CutsceneEnded);
    }

    pub fn camera_locked(&self) -> bool {
        self.camera_locked
    }

    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// Outcome of reconciling a TALKNUM-family message against the installed
/// dialog DAT across client eras.
pub enum FishingChat {
    /// Not a fishing message under any verified hypothesis — the caller falls
    /// back to the plain direct lookup.
    NotFishing,
    /// A verified fishing line: the DAT entry text and the FISHMESSAGEOFFSET
    /// it resolved to.
    Line { text: String, offset: u8 },
    /// Fishing-shaped, but no hypothesis survived verification — the caller
    /// prints the placeholder rather than a guess.
    Unresolved,
}

/// Per-zone era-reconciliation state.
#[derive(Default)]
struct FishingEra {
    /// Landmark-verified fishing-block base of the installed dialog DAT.
    /// `Some(None)` = scanned, block not found.
    install_base: Option<Option<u16>>,
    server: ServerBase,
}

/// The server-side fishing base, learned from the wire.
#[derive(Debug, Default)]
enum ServerBase {
    #[default]
    Unknown,
    /// Surviving candidates, intersected across messages.
    Candidates(Vec<u16>),
    Known(u16),
}

/// How far apart the same zone's fishing base can sit between the installed
/// DAT and the server's text ids and still count as the same block. Measured
/// against the vendored LSB pin's fishing base: KNOWN_CLIENTS horizonxi-2023
/// sits 8-10 entries below it and retail-2026-09 sits 4 above it (LSB
/// origin/base at 30260904_1 matches retail-2026-09 exactly); an older LSB
/// fork sat 26 from horizonxi-2023.
pub const MAX_ERA_SKEW: u16 = 96;

/// Locate the fishing block in an installed dialog DAT by its landmark lines,
/// returning the block's base — the index LSB calls FISHING_MESSAGE_OFFSET.
/// One landmark could collide with another block's duplicate line, so the
/// match requires three lines at their exact relative offsets.
pub fn find_fishing_block(dat: &StringDat) -> Option<u16> {
    use ffxi_proto::fishing_messages::{kind, offset_text};
    let (norod, nocatch, hooked) = (
        offset_text(kind::NOROD)?,
        offset_text(kind::NOCATCH)?,
        offset_text(kind::HOOKED_SMALL_FISH)?,
    );
    let at = |index: usize, probe: &str| dat.text(index).is_some_and(|t| t.starts_with(probe));
    for i in kind::NOROD as usize..dat.len() {
        if !at(i, norod) {
            continue;
        }
        let base = i - kind::NOROD as usize;
        if at(base + kind::NOCATCH as usize, nocatch)
            && at(base + kind::HOOKED_SMALL_FISH as usize, hooked)
        {
            return u16::try_from(base).ok();
        }
    }
    None
}

/// Pick the DAT entry for a TALKNUM-family fishing message. `printable`
/// resolves a FISHMESSAGEOFFSET to the install's entry text, honoring the
/// menu guard. Pure state machine so it can be unit-tested without a DAT.
fn resolve_fishing(
    printable: &dyn Fn(u8) -> Option<String>,
    pin_base: u16,
    install_base: u16,
    mes_num: u16,
    opcode: u16,
    server: ServerBase,
) -> (FishingChat, ServerBase) {
    use ffxi_proto::fishing_messages as fm;

    // The line `base` claims this message is: a known offset the opcode
    // carries, landing on a printable in-block entry.
    let hypothesis = |base: u16| -> Option<(u8, String)> {
        let offset = u8::try_from(mes_num.checked_sub(base)?).ok()?;
        if !fm::carried_by(opcode, offset) {
            return None;
        }
        printable(offset).map(|text| (offset, text))
    };

    // The not-yet-locked path. The server almost always speaks the vendor
    // pin's era (the dev stack) or the installed DAT's own era (a server
    // matched to the player's client); exactly one verified hypothesis locks
    // the zone's base. Both verifying is a skew coincidence, and neither
    // means a third era — those fall through to learning, which intersects
    // the `mes_num - offset` candidates each message implies until one
    // survives.
    let unknown = |candidates: Option<Vec<u16>>| -> (FishingChat, ServerBase) {
        let mut verified = Vec::new();
        for base in [install_base, pin_base] {
            if candidates.as_ref().is_some_and(|c| !c.contains(&base)) {
                continue;
            }
            if verified.iter().any(|&(b, _, _)| b == base) {
                continue;
            }
            if let Some((offset, text)) = hypothesis(base) {
                verified.push((base, offset, text));
            }
        }
        // Catch opcodes distinguish the sparse catch entries; TALKNUM's dense
        // status entries cannot identify an adjacent text-table revision.
        // vendor/server/src/map/utils/fishingutils.cpp CatchFish
        if verified.is_empty() && opcode == ffxi_proto::map::s2c::TALKNUMWORK2 {
            for base in [install_base, pin_base].into_iter().flat_map(|base| {
                [base.checked_sub(1), base.checked_add(1)]
                    .into_iter()
                    .flatten()
            }) {
                if verified.iter().any(|&(b, _, _)| b == base)
                    || candidates.as_ref().is_some_and(|c| !c.contains(&base))
                {
                    continue;
                }
                if let Some((offset, text)) = hypothesis(base) {
                    verified.push((base, offset, text));
                }
            }
        }
        if verified.len() == 1 {
            let (base, offset, text) = verified.pop().expect("len checked");
            return (FishingChat::Line { text, offset }, ServerBase::Known(base));
        }

        let observations: Vec<u16> = fm::OFFSETS
            .iter()
            .copied()
            .filter(|&o| fm::carried_by(opcode, o))
            .filter(|&o| printable(o).is_some())
            .filter_map(|o| mes_num.checked_sub(u16::from(o)))
            .filter(|&b| b.abs_diff(install_base) <= MAX_ERA_SKEW)
            .collect();
        if observations.is_empty() {
            // Nothing about this message says fishing (e.g. a lua-driven
            // messageSpecial): leave any learning state untouched.
            let server = match candidates {
                None => ServerBase::Unknown,
                Some(prev) => ServerBase::Candidates(prev),
            };
            return (FishingChat::NotFishing, server);
        }
        let survivors = match candidates {
            None => observations,
            Some(prev) => {
                let next: Vec<u16> = prev
                    .into_iter()
                    .filter(|b| observations.contains(b))
                    .collect();
                if next.is_empty() {
                    tracing::warn!(
                        mes_num,
                        "fishing era candidates exhausted; restarting learning"
                    );
                    observations
                } else {
                    next
                }
            }
        };
        if let [base] = survivors.as_slice() {
            let base = *base;
            match hypothesis(base) {
                Some((offset, text)) => {
                    (FishingChat::Line { text, offset }, ServerBase::Known(base))
                }
                None => (FishingChat::Unresolved, ServerBase::Known(base)),
            }
        } else {
            (FishingChat::Unresolved, ServerBase::Candidates(survivors))
        }
    };

    match server {
        ServerBase::Known(base) => match hypothesis(base) {
            Some((offset, text)) => (FishingChat::Line { text, offset }, ServerBase::Known(base)),
            // The lock can be a skew coincidence: retry unlocked and adopt a
            // verified rival, but a message that says nothing about fishing
            // (a lua special) leaves the lock alone.
            None => match unknown(None) {
                (FishingChat::NotFishing, _) => (FishingChat::NotFishing, ServerBase::Known(base)),
                retried => retried,
            },
        },
        ServerBase::Unknown => unknown(None),
        ServerBase::Candidates(prev) => unknown(Some(prev)),
    }
}

fn frame_to_dialog(
    active: &ActiveEvent,
    frame: ffxi_event::DialogFrame,
    player_name: &str,
    cancel_armed: bool,
) -> DialogState {
    let ffxi_event::DialogFrame {
        speaker_index,
        text,
        choices,
        params,
        ..
    } = frame;
    let substitute = |text: String| {
        substitute_entity_names(
            substitute_nums(
                substitute_names(text, player_name, active.npc_name.as_deref()),
                &params,
            ),
            &params,
        )
    };
    let item_marker = format!("{{{MARKER_ITEM}:");
    let key_item_marker = format!("{{{MARKER_KEY_ITEM}:");
    let contains_item = text.contains(&item_marker)
        || text.contains(&key_item_marker)
        || choices
            .iter()
            .any(|c| c.contains(&item_marker) || c.contains(&key_item_marker));
    DialogState {
        event_id: active.agent_event_id,
        npc_id: active.unique_no,
        npc_name: active.npc_name.clone(),
        act_index: active.act_index,
        event_num: 0,
        event_para: active.event_id,
        mode: 0,
        event_num2: 0,
        event_para2: 0,
        strings: Vec::new(),
        nums: params.clone(),
        prompt: Some(substitute(text)),
        choices: choices.into_iter().map(substitute).collect(),
        text_entry: false,
        grid: None,
        custom_menu: false,
        cancel_armed,
        speaker_index,
        contains_item,
    }
}

/// Resolve the plain name markers the dmsg decoder leaves in dialog text:
/// `{PlayerName}` → the logged-in character, `{SpeakerName}` → the speaking NPC.
/// A `{SpeakerName}` with no known speaker name is left as-is.
pub fn substitute_names(text: String, player_name: &str, speaker_name: Option<&str>) -> String {
    let text = text.replace(&plain_marker(MARKER_PLAYER_NAME), player_name);
    match speaker_name {
        Some(name) => text.replace(&plain_marker(MARKER_SPEAKER_NAME), name),
        None => text,
    }
}

/// Resolve the parameterized text slots the dmsg decoder leaves as
/// `{ChocoboName:N}` (control code 0x1C — POLUtils' `ChocoboName`, really a
/// generic string slot) with the name the packet carried: the angler on LSB's
/// catch broadcasts. Every index resolves to the same name — zone messages
/// carry just one. `None` leaves the markers visible, like [`substitute_names`]
/// does for an unknown speaker.
pub fn substitute_text_params(text: String, name: Option<&str>) -> String {
    match name {
        Some(name) => {
            substitute_param_marker(text, MARKER_CHOCOBO_NAME, &|_| Some(name.to_string()))
        }
        None => text,
    }
}

/// Resolve the parameterized number markers the dmsg decoder leaves in dialog
/// text: `{Num:N}` → `params[N]` (the event's numeric parameters). A marker
/// whose index is out of range is left as-is so the missing parameter stays
/// visible in fixtures instead of silently printing a wrong value.
pub fn substitute_nums(text: String, params: &[i32]) -> String {
    substitute_param_marker(text, MARKER_NUM, &|index| {
        params.get(index).map(|v| v.to_string())
    })
}

/// Resolve `{KeyItem:N}` / `{Item:N}` (dmsg control codes 0x1a / 0x19):
/// `params[N]` is a key-item / item id looked up in the scraped LSB name
/// tables. Unresolvable markers are left as-is, like [`substitute_nums`].
pub fn substitute_entity_names(text: String, params: &[i32]) -> String {
    let text = substitute_param_marker(text, MARKER_KEY_ITEM, &|index| {
        let id = u16::try_from(*params.get(index)?).ok()?;
        ffxi_vocab::key_item_names::lookup(id).map(str::to_string)
    });
    substitute_param_marker(text, MARKER_ITEM, &|index| {
        let id = u16::try_from(*params.get(index)?).ok()?;
        ffxi_vocab::item_names::lookup(id).map(str::to_string)
    })
}

/// [`substitute_entity_names`] keeping the substitution boundary, so the item /
/// key-item name can be coloured apart from the text around it. Retail renders
/// it as its own green run — the boundary is exactly the substitution slot,
/// excluding the article before it and the punctuation after
/// (`.agents/skills/retail-observe/references/treasure-pool-chat.md`).
pub fn spanned_entity_names(text: &str, params: &[i32]) -> Vec<ffxi_dat::sysmes::Span> {
    use ffxi_dat::sysmes::{Span, SpanKind};

    const MARKERS: [(&str, SpanKind); 2] = [
        (MARKER_KEY_ITEM, SpanKind::KeyItem),
        (MARKER_ITEM, SpanKind::Item),
    ];
    let opens: Vec<(String, SpanKind)> = MARKERS
        .iter()
        .map(|(m, k)| (format!("{{{m}:"), *k))
        .collect();

    let mut spans: Vec<Span> = Vec::new();
    let push_text = |spans: &mut Vec<Span>, text: &str| {
        if text.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some(last) if last.kind == SpanKind::Text => last.text.push_str(text),
            _ => spans.push(Span {
                text: text.to_string(),
                kind: SpanKind::Text,
            }),
        }
    };

    let mut rest = text;
    loop {
        let next = opens
            .iter()
            .filter_map(|(open, kind)| rest.find(open.as_str()).map(|at| (at, open, *kind)))
            .min_by_key(|(at, _, _)| *at);
        let Some((at, open, kind)) = next else { break };

        let after_open = &rest[at + open.len()..];
        let resolved = after_open.find('}').and_then(|end| {
            let index: usize = after_open[..end].parse().ok()?;
            let id = u16::try_from(*params.get(index)?).ok()?;
            let name = match kind {
                SpanKind::KeyItem => ffxi_vocab::key_item_names::lookup(id),
                _ => ffxi_vocab::item_names::lookup(id),
            }?;
            Some((name.to_string(), end + 1))
        });

        push_text(&mut spans, &rest[..at]);
        match resolved {
            Some((name, consumed)) => {
                spans.push(Span { text: name, kind });
                rest = &after_open[consumed..];
            }
            // Unresolvable marker: left verbatim, exactly like the plain
            // substitution, so a missing name stays visible instead of
            // silently vanishing.
            None => {
                push_text(&mut spans, open);
                rest = after_open;
            }
        }
    }
    push_text(&mut spans, rest);
    spans
}

fn substitute_param_marker(
    text: String,
    marker: &str,
    resolve: &dyn Fn(usize) -> Option<String>,
) -> String {
    let open = format!("{{{marker}:");
    if !text.contains(&open) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find(&open) {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let resolved = rest[open.len()..].find('}').and_then(|end| {
            let index: usize = rest[open.len()..open.len() + end].parse().ok()?;
            let value = resolve(index)?;
            Some((value, open.len() + end + 1))
        });
        match resolved {
            Some((value, consumed)) => {
                out.push_str(&value);
                rest = &rest[consumed..];
            }
            None => {
                out.push_str(&open);
                rest = &rest[open.len()..];
            }
        }
    }
    out.push_str(rest);
    out
}

// The load failures below are logged once per zone: `ensure_event_dat` /
// `ensure_strings` only call these when their loaded zone changes, and cache the
// (None) result (kuluu-zkuf).

fn load_event_dat(root: Option<&DatRoot>, zone: u16) -> Option<EventDat> {
    let root = root?;
    let loc = match root.resolve(ffxi_dat::event_locate::event_dat_file_id(zone)) {
        Ok(loc) => loc,
        Err(e) => {
            tracing::warn!(
                zone,
                error = %e,
                "no event DAT for zone; NPC dialog disabled for this zone"
            );
            return None;
        }
    };
    let path = loc.path_under(root);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                zone,
                path = %path.display(),
                error = %e,
                "failed to read event DAT; NPC dialog disabled for this zone"
            );
            return None;
        }
    };
    match EventDat::parse(&bytes) {
        Ok(dat) => Some(dat),
        Err(e) => {
            tracing::warn!(
                zone,
                path = %path.display(),
                error = %e,
                "failed to parse event DAT; NPC dialog disabled for this zone"
            );
            None
        }
    }
}

// 0x45 duration operand: 0 and this value mean "play the authored timing";
// kuluu-render/src/cutscene.rs scheduler_speed_ratio treats both as ratio 1.
const SCHEDULER_DURATION_LOOP: u16 = 1;

/// The VM's wait clock: one hold unit per 1/60 s (ffxi-event vm.rs
/// WAIT_UNITS_PER_SEC).
const WAIT_UNITS_PER_SEC: f32 = 60.0;

/// Deadline for a motion hold the renderer does not report (stopped routine,
/// despawned entity, renderer-less session) when the session cannot read the
/// routine's authored length: well above any model-DAT routine length, so a
/// live finish report releases the hold ahead of the sweep.
const PENDING_MOTION_HOLD_MAX: std::time::Duration = std::time::Duration::from_secs(15);

/// Arm the MOVE case-1 hold for each walk in `raw_cues`: the length comes
/// from this session's own entity positions and the authored speed, because
/// the VM never measures a move (research/XiEvents/OpCodes/0x001F.md). An
/// unknown position or a zero speed arms nothing, so the wait falls through.
fn arm_move_holds(
    runner: &mut DialogRunner,
    raw_cues: &[EventCue],
    positions: &std::collections::HashMap<u32, ffxi_event::vm::scene::EventPosition>,
    event_entity: u32,
) {
    for cue in raw_cues {
        let EventCue::ActorMove { actor, goal, speed } = *cue else {
            continue;
        };
        let server_id = if actor.is_event_entity() {
            event_entity
        } else if let Some(id) = actor.server_id() {
            id
        } else {
            tracing::debug!(
                target: "kuluu_session::event_dialog",
                "move cue names no resolvable actor; the MOVE hold falls through"
            );
            continue;
        };
        let Some(current) = positions.get(&server_id) else {
            tracing::debug!(
                target: "kuluu_session::event_dialog",
                server_id,
                "no known position for the moving actor; the MOVE hold falls through"
            );
            continue;
        };
        if speed <= 0 {
            tracing::debug!(
                target: "kuluu_session::event_dialog",
                server_id,
                speed,
                "move cue carries no authored speed; the MOVE hold falls through"
            );
            continue;
        }
        // The scene lerp travels `speed * EVENT_SPEED_SCALE * EVENT_COORD_UNITS`
        // event units per second in the x/z plane (ffxi-event vm/scene.rs
        // tick_scene); hold units are 1/60 s on the VM's wait clock.
        let dx = (goal.x - current.x) as f32;
        let dz = (goal.z - current.z) as f32;
        let yalms_per_sec = speed as f32 * ffxi_event::vm::scene::EVENT_SPEED_SCALE;
        let units = dx.hypot(dz) / (yalms_per_sec * ffxi_event::vm::scene::EVENT_COORD_UNITS)
            * WAIT_UNITS_PER_SEC;
        tracing::debug!(
            target: "kuluu_session::event_dialog",
            server_id,
            speed,
            hold_secs = units / WAIT_UNITS_PER_SEC,
            "armed the MOVE hold from the session's own entity positions"
        );
        runner.hold_move(actor, units);
    }
}

/// Arm WAIT* holds for motion cues, before their actors are resolved: the hold
/// keys on the VM's own unresolved ActorLookup. Every routine the renderer
/// plays and reports (0x2C, 0x45 non-fade, 0x5B/0x66, 0x2D) arms a pending
/// hold the renderer's finish report releases; the DAT-authored routine length
/// is its deadline, so a renderer-less session times out on the authored
/// length itself (the deadline sweep in [`DialogSession::tick`] is the last
/// resort). Only the 0x45 fades, which cutscene.rs plays without reporting,
/// keep a timed hold.
fn arm_motion_holds(
    runner: &mut DialogRunner,
    raw_cues: &[EventCue],
    root: Option<&DatRoot>,
    cache: &mut std::collections::HashMap<(u32, FourCc), Option<f32>>,
    event_entity: u32,
    zone: u16,
    pending: &mut std::collections::HashMap<
        (CutsceneActor, FourCc),
        (ActorLookup, u32, std::time::Instant, std::time::Duration),
    >,
) {
    for cue in raw_cues {
        match *cue {
            // 0x2C: the routine is in the actor's own model DAT, which this
            // session does not open: no DAT length, so [`PENDING_MOTION_HOLD_MAX`] stands.
            EventCue::ActorMotion { actor1, key, .. } => {
                arm_pending_motion_hold(
                    runner,
                    pending,
                    actor1,
                    resolve_actor(actor1, event_entity),
                    key,
                    None,
                );
            }
            EventCue::Scheduler {
                dat_id,
                actor1,
                tag,
                duration,
                ..
            } => {
                let units = routine_units(root, cache, dat_id, tag, duration);
                if dat_id == ffxi_event::SCHEDULER_FADE_DAT_ID {
                    // The fade plays in cutscene.rs, which reports no finish:
                    // its hold stays timed from the DAT length.
                    if let Some(units) = units {
                        runner.hold_action(actor1, tag, units);
                    }
                } else {
                    arm_pending_motion_hold(
                        runner,
                        pending,
                        actor1,
                        resolve_actor(actor1, event_entity),
                        tag,
                        units,
                    );
                }
            }
            EventCue::ExtScheduler {
                motion,
                actor1,
                key,
                ..
            } => {
                // The routine's schedulers live in container A (tag 1), which
                // both the 0x5B single file and the 0x66 package name. An
                // out-of-range 0x66 package names no container: the renderer
                // plays nothing and reports nothing, so arm nothing.
                let file_id = match motion {
                    Some(ffxi_event::ExtSchedulerMotion::Event(id)) => Some(id),
                    Some(ffxi_event::ExtSchedulerMotion::Tpc(pkgs)) => Some(pkgs.a),
                    None => None,
                };
                let Some(file_id) = file_id else {
                    continue;
                };
                arm_pending_motion_hold(
                    runner,
                    pending,
                    actor1,
                    resolve_actor(actor1, event_entity),
                    key,
                    routine_units(
                        root,
                        cache,
                        file_id,
                        key,
                        ffxi_event::SCHEDULER_DURATION_FROM_DAT,
                    ),
                );
            }
            // 0x2D: retail runs the routine out of the CURRENT zone's own model DAT
            // (the cue carries the zone id); on a miss, the entrance/instance partner
            // zone's model DAT, then the non-model scene carriers. The hold arms from
            // the file the key resolved in; retail waits on it via the zone object, so
            // the VM's hold keys on the zone sentinel while the renderer's report keys
            // on the cue's actor1.
            EventCue::ZoneScheduler { key, actor1, .. } => {
                let units = root
                    .and_then(|root| ffxi_dat::scheduler::zone_scene_file_id(root, zone, key))
                    .and_then(|file_id| {
                        routine_units(
                            root,
                            cache,
                            file_id,
                            key,
                            ffxi_event::SCHEDULER_DURATION_FROM_DAT,
                        )
                    });
                arm_pending_motion_hold(
                    runner,
                    pending,
                    ffxi_event::ActorLookup::ZONE,
                    resolve_actor(actor1, event_entity),
                    key,
                    units,
                );
            }
            _ => {}
        }
    }
}

/// Arm the pending hold for a motion cue the renderer plays and reports: the
/// renderer's finish report releases it, and the DAT-authored routine length
/// (in WAIT* units) is its deadline, so a renderer-less session times out on
/// the authored length itself. An unreadable DAT falls back to
/// [`PENDING_MOTION_HOLD_MAX`]. `wire` is the cue's own resolved actor - the
/// value the renderer's report carries back - which for the 0x2D zone
/// sentinel is not the VM's hold key.
fn arm_pending_motion_hold(
    runner: &mut DialogRunner,
    pending: &mut std::collections::HashMap<
        (CutsceneActor, FourCc),
        (ActorLookup, u32, std::time::Instant, std::time::Duration),
    >,
    lookup: ActorLookup,
    wire: CutsceneActor,
    key: FourCc,
    units: Option<f32>,
) {
    runner.hold_action_pending(lookup, key);
    let deadline = units
        .map(|units| std::time::Duration::from_secs_f32(units / WAIT_UNITS_PER_SEC))
        .unwrap_or(PENDING_MOTION_HOLD_MAX);
    let entry =
        pending
            .entry((wire, key))
            .or_insert((lookup, 0, std::time::Instant::now(), deadline));
    entry.1 += 1;
    entry.2 = std::time::Instant::now();
}

/// The authored length of scheduler `tag` in DAT file `dat_id`, in WAIT* hold
/// units (1/60 s each; the routine clock and the VM's wait clock are both 60
/// fps). `duration_override` is the 0x45 operand: 0 or 1 means play the
/// authored timing, anything else IS the total frame count. Cached per
/// (dat_id, tag) with misses included so a re-issued routine does not
/// re-read its file; a missing DAT arms nothing and the wait falls through.
fn routine_units(
    root: Option<&DatRoot>,
    cache: &mut std::collections::HashMap<(u32, FourCc), Option<f32>>,
    dat_id: u32,
    tag: FourCc,
    duration_override: u16,
) -> Option<f32> {
    let units = if let Some(units) = cache.get(&(dat_id, tag)) {
        *units
    } else {
        let units = routine_units_uncached(root, dat_id, tag, duration_override);
        cache.insert((dat_id, tag), units);
        units
    };
    if let Some(units) = &units {
        tracing::debug!(
            target: "kuluu_session::event_dialog",
            dat_id,
            tag = %String::from_utf8_lossy(&tag),
            hold_secs = units / WAIT_UNITS_PER_SEC,
            "armed the WAIT* hold from the DAT-authored routine length"
        );
    }
    units
}

fn routine_units_uncached(
    root: Option<&DatRoot>,
    dat_id: u32,
    tag: FourCc,
    duration_override: u16,
) -> Option<f32> {
    let miss = |reason: &str| {
        tracing::debug!(
            target: "kuluu_session::event_dialog",
            dat_id,
            tag = %String::from_utf8_lossy(&tag),
            reason,
            "no authored routine length; the WAIT* hold falls through"
        );
    };
    let Some(root) = root else {
        miss("no DAT root");
        return None;
    };
    let loc = match root.resolve(dat_id) {
        Ok(loc) => loc,
        Err(e) => {
            tracing::debug!(
                target: "kuluu_session::event_dialog",
                dat_id,
                tag = %String::from_utf8_lossy(&tag),
                error = %e,
                "failed to resolve the motion DAT; the WAIT* hold falls through"
            );
            return None;
        }
    };
    let path = loc.path_under(root);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(
                target: "kuluu_session::event_dialog",
                dat_id,
                tag = %String::from_utf8_lossy(&tag),
                path = %path.display(),
                error = %e,
                "failed to read the motion DAT; the WAIT* hold falls through"
            );
            return None;
        }
    };
    let chunk = ffxi_dat::chunk::walk(&bytes).find_map(|c| match c {
        Ok(c) if c.kind == ChunkKind::Scheduler as u8 && c.name == tag => Some(c),
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(
                target: "kuluu_session::event_dialog",
                dat_id,
                error = %e,
                "truncated chunk while scanning the motion DAT"
            );
            None
        }
    })?;
    let scheduler = match Scheduler::parse(chunk.name, chunk.data) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(
                target: "kuluu_session::event_dialog",
                dat_id,
                tag = %String::from_utf8_lossy(&tag),
                error = %e,
                "failed to parse the scheduler chunk; the WAIT* hold falls through"
            );
            return None;
        }
    };
    let frames = if duration_override <= SCHEDULER_DURATION_LOOP {
        scheduler.end_frame()
    } else {
        u32::from(duration_override)
    };
    Some(frames as f32)
}

fn load_strings(root: Option<&DatRoot>, zone: u16) -> Option<StringDat> {
    let root = root?;
    let file_id = ffxi_dat::zone_dat::string_dat_file_id(zone);
    let loc = match root.resolve(file_id) {
        Ok(loc) => loc,
        Err(e) => {
            tracing::warn!(
                zone,
                file_id,
                error = %e,
                "failed to resolve string DAT file id; NPC dialog disabled for this zone"
            );
            return None;
        }
    };
    let path = loc.path_under(root);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                zone,
                path = %path.display(),
                error = %e,
                "failed to read string DAT; NPC dialog disabled for this zone"
            );
            return None;
        }
    };
    match StringDat::parse(&bytes) {
        Ok(dat) => Some(dat),
        Err(e) => {
            tracing::warn!(
                zone,
                path = %path.display(),
                error = %e,
                "failed to parse string DAT; NPC dialog disabled for this zone"
            );
            None
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A miniature fishing block: offsets relative to a base, mirroring the
    /// real layout's landmark lines.
    struct FakeDat {
        lines: Vec<(u8, &'static str)>,
    }

    impl FakeDat {
        /// The landmarks `find_fishing_block` keys on, plus the lines the
        /// tests exercise.
        fn new() -> Self {
            Self {
                lines: vec![
                    (0x01, "You can't fish without a rod in your hands."),
                    (0x04, "You didn't catch anything."),
                    (0x08, "Something caught the hook!"),
                    (
                        0x11,
                        "Your rod breaks. Whatever caught the hook was pretty big.",
                    ),
                    (0x27, "{ChocoboName:0} caught  {Item:0}!"),
                    (0x0E, "{ChocoboName:0} caught {Num:1} {Item:0}!"),
                ],
            }
        }

        fn printable(&self) -> impl Fn(u8) -> Option<String> + '_ {
            |offset| {
                self.lines
                    .iter()
                    .find(|(o, _)| *o == offset)
                    .map(|(_, t)| t.to_string())
            }
        }
    }

    use ffxi_proto::fishing_messages::kind;
    use ffxi_proto::map::s2c;

    /// The vendor pin's base sits 9 above a May-2023 install's, the skew the
    /// era reconciliation exists for.
    const PIN: u16 = 7258;
    const INSTALL: u16 = 7249;

    fn line_text(chat: &FishingChat) -> Option<&str> {
        match chat {
            FishingChat::Line { text, .. } => Some(text),
            _ => None,
        }
    }

    /// Dev stack: a pin-era server against a May-2023 install. A catch
    /// announcement verifies against the pin base alone and locks it.
    #[test]
    fn pin_era_server_locks_on_the_first_catch() {
        let dat = FakeDat::new();
        let mes = PIN + kind::CATCH as u16;
        let (chat, server) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            mes,
            s2c::TALKNUMWORK2,
            ServerBase::Unknown,
        );
        assert_eq!(line_text(&chat), Some("{ChocoboName:0} caught  {Item:0}!"));
        assert!(matches!(server, ServerBase::Known(PIN)));

        // Once locked, a message whose direct index would have been another
        // line entirely (install[PIN + NOCATCH - INSTALL] = offset 13, not the
        // NOCATCH line) renders the shifted NOCATCH line.
        let (chat, _) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            PIN + kind::NOCATCH as u16,
            s2c::TALKNUM,
            server,
        );
        assert_eq!(line_text(&chat), Some("You didn't catch anything."));
    }

    #[test]
    fn ferry_catch_from_adjacent_server_revision_resolves_before_local_fishing() {
        const FERRY_PIN: u16 = 7250;
        const FERRY_INSTALL: u16 = 7241;
        const FERRY_SERVER: u16 = 7249;
        const REPORTED_CATCH: u16 = 7288;
        let dat = FakeDat::new();
        let (chat, server) = resolve_fishing(
            &dat.printable(),
            FERRY_PIN,
            FERRY_INSTALL,
            REPORTED_CATCH,
            s2c::TALKNUMWORK2,
            ServerBase::Unknown,
        );
        assert_eq!(line_text(&chat), Some("{ChocoboName:0} caught  {Item:0}!"));
        assert!(matches!(server, ServerBase::Known(FERRY_SERVER)));
        let (chat, _) = resolve_fishing(
            &dat.printable(),
            FERRY_PIN,
            FERRY_INSTALL,
            FERRY_SERVER + u16::from(kind::NOCATCH),
            s2c::TALKNUM,
            server,
        );
        assert_eq!(line_text(&chat), Some("You didn't catch anything."));
    }

    /// An era-matched server (base == install's) keeps rendering directly,
    /// and locks on the first message.
    #[test]
    fn era_matched_server_renders_directly() {
        let dat = FakeDat::new();
        let (chat, server) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            INSTALL + kind::NOCATCH as u16,
            s2c::TALKNUM,
            ServerBase::Unknown,
        );
        assert_eq!(line_text(&chat), Some("You didn't catch anything."));
        assert!(matches!(server, ServerBase::Known(INSTALL)));
    }

    /// A skew coincidence — the pin hypothesis and the install hypothesis both
    /// land on real lines — must not guess: placeholder and keep learning.
    /// (HOOKED_SMALL at skew 9 aliases RODBREAK_TOOBIG.)
    #[test]
    fn a_skew_coincidence_is_unresolved_not_guessed() {
        let dat = FakeDat::new();
        let (chat, server) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            PIN + kind::HOOKED_SMALL_FISH as u16,
            s2c::TALKNUM,
            ServerBase::Unknown,
        );
        assert!(matches!(chat, FishingChat::Unresolved));
        assert!(matches!(server, ServerBase::Candidates(_)));
    }

    /// A server from a third era (base matching neither pin nor install) is
    /// learned by intersecting the candidates each message implies: an
    /// ambiguous catch, then any TALKNUM line, converges.
    #[test]
    fn a_third_era_server_base_is_learned_from_the_wire() {
        const SERVER: u16 = 7220; // matches neither PIN nor INSTALL
        let dat = FakeDat::new();

        // A catch broadcast: CATCH and CATCH_INV_FULL/CATCH_MULTI candidates.
        let (chat, server) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            SERVER + kind::CATCH as u16,
            s2c::TALKNUMWORK2,
            ServerBase::Unknown,
        );
        assert!(matches!(chat, FishingChat::Unresolved));

        // Any single-offset message intersects the candidates to a singleton.
        let (chat, server) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            SERVER + kind::NOCATCH as u16,
            s2c::TALKNUM,
            server,
        );
        assert_eq!(line_text(&chat), Some("You didn't catch anything."));
        assert!(matches!(server, ServerBase::Known(SERVER)));

        // The catch that previously had to wait now renders.
        let (chat, _) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            SERVER + kind::CATCH as u16,
            s2c::TALKNUMWORK2,
            server,
        );
        assert_eq!(line_text(&chat), Some("{ChocoboName:0} caught  {Item:0}!"));
    }

    /// A catch broadcast on a third-era server narrows the base to the
    /// catch-family candidates; the next single-offset line intersects them
    /// to a singleton.
    #[test]
    fn a_multi_catch_narrows_then_a_talknum_locks() {
        const SERVER: u16 = 7220;
        let dat = FakeDat::new();
        let (chat, server) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            SERVER + kind::CATCH_MULTI as u16,
            s2c::TALKNUMWORK2,
            ServerBase::Unknown,
        );
        assert!(matches!(chat, FishingChat::Unresolved));
        let ServerBase::Candidates(c) = &server else {
            panic!("multi-catch must narrow, not lock: {server:?}");
        };
        assert!(c.contains(&SERVER) && c.len() >= 2, "candidates: {c:?}");

        let (chat, server) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            SERVER + kind::NOCATCH as u16,
            s2c::TALKNUM,
            server,
        );
        assert_eq!(line_text(&chat), Some("You didn't catch anything."));
        assert!(matches!(server, ServerBase::Known(SERVER)));
    }

    /// A message with no fishing-shaped interpretation (a lua messageSpecial)
    /// is NotFishing and must not disturb learning state.
    #[test]
    fn non_fishing_messages_do_not_disturb_learning() {
        let dat = FakeDat::new();
        let (chat, server) = resolve_fishing(
            &dat.printable(),
            PIN,
            INSTALL,
            42,
            s2c::TALKNUMWORK,
            ServerBase::Unknown,
        );
        assert!(matches!(chat, FishingChat::NotFishing));
        assert!(matches!(server, ServerBase::Unknown));
    }

    /// Build a synthetic DialogTable from per-entry (already-plain) text
    /// bytes. Mirrors ffxi_dat::dmsg's own test helper; the format constants
    /// duplicate `StringDat::parse`'s (TEXT_XOR / OFFSET_XOR / MAGIC_BASE),
    /// which are pub(crate) to ffxi-dat — ffxi-dat's tests pin the format.
    fn synth_dat(entries: &[&[u8]]) -> Vec<u8> {
        const STRING_DAT_MAGIC_BASE: u32 = 0x1000_0000;
        const STRING_DAT_OFFSET_XOR: u32 = 0x8080_8080;
        const STRING_DAT_TEXT_XOR: u8 = 0x80;
        let count = entries.len();
        let table_size = 4 * count;
        let mut offsets = Vec::with_capacity(count);
        let mut running = table_size as u32;
        for e in entries {
            offsets.push(running);
            running += e.len() as u32;
        }
        let data_len = table_size as u32 + entries.iter().map(|e| e.len() as u32).sum::<u32>();
        let mut buf = Vec::new();
        buf.extend_from_slice(&(STRING_DAT_MAGIC_BASE.wrapping_add(data_len)).to_le_bytes());
        for off in &offsets {
            buf.extend_from_slice(&(off ^ STRING_DAT_OFFSET_XOR).to_le_bytes());
        }
        for e in entries {
            buf.extend(e.iter().map(|b| b ^ STRING_DAT_TEXT_XOR));
        }
        buf
    }

    pub(crate) fn contract_session(
        dat: EventDat,
        event_zone: u16,
        text_zone: u16,
    ) -> DialogSession {
        let mut session = DialogSession::new(None, "Test".into());
        session.loaded_event_zone = Some(event_zone);
        session.loaded_string_zone = Some(text_zone);
        session.event_dat = Some(Arc::new(dat));
        session.strings = Some(
            StringDat::parse(&synth_dat(&[
                b"Balance {Num:0}, fare {Num:1}\0",
                b"Accepted: {Num:0}, fare {Num:1}\0",
                b"Insufficient: {Num:0}, fare {Num:1}\0",
            ]))
            .unwrap(),
        );
        session
    }

    fn position_update_session() -> (DialogSession, EventTrigger) {
        const ZONE: u16 = 248;
        const ACTOR: u32 = 17_793_078;
        const EVENT: u16 = 221;
        let block = ffxi_dat::event_dat::EventBlock {
            actor: ACTOR,
            event_ids: vec![EVENT],
            event_offsets: vec![0],
            references: vec![33_762, (-31_432i32) as u32, (-2_558i32) as u32, 0],
            event_data: vec![0x47, 0, 0, 0x80, 1, 0x80, 2, 0x80, 3, 0x80, 0x47, 1, 0x21],
        };
        let mut session = DialogSession::new(None, "Test".into());
        session.loaded_event_zone = Some(ZONE);
        session.loaded_string_zone = Some(ZONE);
        session.event_dat = Some(Arc::new(EventDat {
            blocks: vec![block],
        }));
        session.strings = Some(StringDat::parse(&synth_dat(&[b"test"])).unwrap());
        session.set_player_position(ffxi_event::vm::scene::EventPosition::default());
        let trigger = EventTrigger {
            event_zone: ZONE,
            text_zone: ZONE,
            unique_no: ACTOR,
            act_index: 54,
            event_id: EVENT,
            params: vec![],
            npc_name: None,
        };
        (session, trigger)
    }

    /// Event 503's master block runs 0x42 as its second opcode (the VM's cancel
    /// disarm). The first dialog frame must carry cancel_armed=false so the
    /// client's ESC gate holds; a program without it stays cancellable.
    #[test]
    fn cancel_disarm_opcodes_flow_into_the_first_frame() {
        const NPC: u32 = 0x010E_6032;
        const EVENT: u16 = 503;
        const ZONE: u16 = 248;

        fn begin_with(program: Vec<u8>) -> Begin {
            let block = ffxi_dat::event_dat::EventBlock {
                actor: NPC,
                event_ids: vec![EVENT],
                event_offsets: vec![0],
                references: vec![900],
                event_data: program,
            };
            let mut session = DialogSession::new(None, "Test".into());
            session.loaded_event_zone = Some(ZONE);
            session.loaded_string_zone = Some(ZONE);
            session.event_dat = Some(Arc::new(EventDat {
                blocks: vec![block],
            }));
            session.strings = Some(StringDat::parse(&synth_dat(&[b"test"])).unwrap());
            let trigger = EventTrigger {
                event_zone: ZONE,
                text_zone: ZONE,
                unique_no: NPC,
                act_index: 54,
                event_id: EVENT,
                params: vec![],
                npc_name: None,
            };
            session.begin(trigger)
        }

        // 0x42 disarm, then a chat line (0x1D, ref[0]) + MESWAIT.
        let Begin::Frame(dialog) = begin_with(vec![0x42, 0x1D, 0x00, 0x80, 0x23, 0x21]) else {
            panic!("the disarmed program produced no frame");
        };
        assert!(!dialog.cancel_armed);
        // The trigger's target index rides the frame as its speaker.
        assert_eq!(dialog.speaker_index, Some(54));

        // Same line without the disarm opcode stays cancellable.
        let Begin::Frame(dialog) = begin_with(vec![0x1D, 0x00, 0x80, 0x23, 0x21]) else {
            panic!("the armed program produced no frame");
        };
        assert!(dialog.cancel_armed);
    }

    /// Event 503's map beat (master +045D9..+04606): the coupon line parks on its MESWAIT
    /// with the map open, and CLOSE_MAP only runs after dismissal. The up→down edge detector
    /// must fire exactly once — on the step that dismisses the parked frame into a timed hold —
    /// and never again while the scene stays parked: ticks parked with the frame still up also
    /// report Waiting, so a plain Advance::Waiting cannot carry this signal.
    #[test]
    fn message_closed_fires_exactly_once_per_up_down_transition() {
        const NPC: u32 = 0x010E_6032;
        const EVENT: u16 = 503;
        const ZONE: u16 = 248;

        let block = ffxi_dat::event_dat::EventBlock {
            actor: NPC,
            event_ids: vec![EVENT],
            event_offsets: vec![0],
            // refs[0] is the string index; refs[1] a long WAIT in 1/60s units so the
            // post-dismissal hold outlives every tick this test runs.
            references: vec![900, 60 * 100],
            event_data: vec![
                0x1D, 0x00, 0x80, // MESSAGE refs[0]
                0x23, // MESWAIT
                0x1C, 0x01, 0x80, // WAIT refs[1] (timed hold after dismissal)
                0x21, // END
            ],
        };
        let mut session = DialogSession::new(None, "Test".into());
        session.loaded_event_zone = Some(ZONE);
        session.loaded_string_zone = Some(ZONE);
        session.event_dat = Some(Arc::new(EventDat {
            blocks: vec![block],
        }));
        session.strings = Some(StringDat::parse(&synth_dat(&[b"test"])).unwrap());
        let trigger = EventTrigger {
            event_zone: ZONE,
            text_zone: ZONE,
            unique_no: NPC,
            act_index: 54,
            event_id: EVENT,
            params: vec![],
            npc_name: None,
        };

        // First frame up: the detector syncs on this call, nothing has closed yet.
        let Begin::Frame(_dialog) = session.begin(trigger) else {
            panic!("the program produced no first frame");
        };
        assert!(
            !session.take_message_closed(),
            "no close before any dismissal"
        );

        // Parked on the MESWAIT with the frame still displayed: ticks report Waiting and
        // must not fire the detector.
        for _ in 0..3 {
            let Advance::Waiting = session.tick(1.0 / 60.0) else {
                panic!("parked tick should wait");
            };
            assert!(
                !session.take_message_closed(),
                "frame still up, nothing closed"
            );
        }

        // Dismissal: the box hides and the scene parks on its timed WAIT — exactly one fire.
        let Advance::Waiting = session.advance(None) else {
            panic!("dismissal should park on the hold");
        };
        assert!(
            session.take_message_closed(),
            "the dismissal must fire once"
        );

        // Further ticks parked after it: no re-fire.
        for _ in 0..3 {
            let Advance::Waiting = session.tick(1.0 / 60.0) else {
                panic!("parked tick should wait");
            };
            assert!(!session.take_message_closed(), "no re-fire while parked");
        }
    }

    /// A minimal DAT root that resolves `dat_id` to ROM/21/39.DAT, which holds
    /// one scheduler chunk named `tag`: a single zero-delay stage of duration
    /// `frames`, so the authored end frame is exactly `frames`.
    fn motion_dat_root(dat_id: u32, tag: [u8; 4], frames: u16) -> (tempfile::TempDir, DatRoot) {
        let dir = tempfile::tempdir().unwrap();
        // VTABLE.DAT: byte[file_id] is the ROM index that claims it.
        let mut vtable = vec![0u8; dat_id as usize + 1];
        vtable[dat_id as usize] = 1;
        std::fs::write(dir.path().join("VTABLE.DAT"), &vtable).unwrap();
        // FTABLE.DAT: one u16 word per file id, dir << 7 | file.
        let mut ftable = vec![0u16; dat_id as usize + 1];
        ftable[dat_id as usize] = (21u16) << 7 | 39;
        std::fs::write(
            dir.path().join("FTABLE.DAT"),
            ftable
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        // Chunk: a 64-byte scheduler header whose section table points at one
        // stage, that stage (0x59 AnimationLock, delay 0, duration `frames`),
        // then the end-of-section opcode.
        let mut body = vec![0u8; 64];
        // The section table measures offsets from the chunk header
        // (ffxi-dat/src/scheduler.rs effect_section_start), so the stage at body
        // offset 64 sits at raw 80.
        body[0x14..0x18].copy_from_slice(&80u32.to_le_bytes());
        body.extend([0x59, 0x02, 0x00, 0x00]); // stage opcode + length (8 bytes)
        body.extend(0u16.to_le_bytes()); // delay
        body.extend(frames.to_le_bytes()); // duration
                                           // Retail chunks are a 16-byte header plus a body on the same stride; the
                                           // walker (ffxi-dat/src/chunk.rs walk) reads the name and size word out of the first
                                           // eight bytes and starts the body at offset 16, so the header's remaining
                                           // eight bytes must be written even though they are zero.
        const CHUNK_STRIDE: usize = 16;
        let total = CHUNK_STRIDE + body.len();
        let padded = total.div_ceil(CHUNK_STRIDE) * CHUNK_STRIDE;
        let mut chunk = Vec::with_capacity(padded);
        chunk.extend_from_slice(&tag);
        chunk.extend_from_slice(
            &(((padded / CHUNK_STRIDE) as u32) << 7 | ffxi_dat::ChunkKind::Scheduler as u32)
                .to_le_bytes(),
        );
        chunk.resize(CHUNK_STRIDE, 0);
        chunk.extend(body);
        chunk.resize(padded, 0);
        let rom = dir.path().join("ROM").join("21");
        std::fs::create_dir_all(&rom).unwrap();
        std::fs::write(rom.join("39.DAT"), &chunk).unwrap();
        let root = DatRoot::open(dir.path()).unwrap();
        (dir, root)
    }

    #[test]
    fn extscheduler_hold_parks_until_the_renderer_reports_done() {
        const NPC: u32 = 0x010E_6032;
        const EVENT: u16 = 503;
        const ZONE: u16 = 248;
        const KEY: [u8; 4] = *b"abcd";
        // 0x5B file operand 5 -> band 0 -> dat id 32109.
        let (dat_id, frames) = (32_109u32, 60u16);
        let (_dir, root) = motion_dat_root(dat_id, KEY, frames);
        let mut program = vec![0x5B];
        program.extend(0x8000u16.to_le_bytes()); // file @1 -> references[0]
        program.extend(NPC.to_le_bytes()); // actor1 @3
        program.extend(0u32.to_le_bytes()); // actor2 @7
        program.extend(KEY); // key @11
        program.push(0x53); // WAITSCHEDULOR @15
        program.extend(NPC.to_le_bytes()); // actor1 @16
        program.extend(0u32.to_le_bytes()); // actor2 @20
        program.extend(KEY); // key @24
        program.push(0x21); // END @28
        let block = ffxi_dat::event_dat::EventBlock {
            actor: NPC,
            event_ids: vec![EVENT],
            event_offsets: vec![0],
            references: vec![5],
            event_data: program,
        };
        let mut session = DialogSession::new(Some(Arc::new(root)), "Test".into());
        session.loaded_event_zone = Some(ZONE);
        session.loaded_string_zone = Some(ZONE);
        session.event_dat = Some(Arc::new(EventDat {
            blocks: vec![block],
        }));
        session.strings = Some(StringDat::parse(&synth_dat(&[b"test"])).unwrap());
        // The 0x5B gate loads only for entity Type {1,2,7,8}; feed the NPC's
        // Type the way a 0x0E CHAR_NPC would, under its target index (the
        // server id's low 10 bits, retail's GetActorIndex).
        session.note_entity_type(NPC, (NPC & 0x3FF) as u16, 2);
        let trigger = EventTrigger {
            event_zone: ZONE,
            text_zone: ZONE,
            unique_no: NPC,
            act_index: 0,
            event_id: EVENT,
            params: vec![],
            npc_name: None,
        };
        assert!(matches!(session.begin(trigger), Begin::Waiting));
        let cues = session.take_cues();
        let [ResolvedCue::Scene(CutsceneCue::ExtScheduler {
            motion, actor, key, ..
        })] = cues.as_slice()
        else {
            panic!("expected the ExtScheduler cue");
        };
        assert_eq!(
            *motion,
            Some(kuluu_snapshot::ExtSchedulerMotion::Event(dat_id))
        );
        assert_eq!(*actor, CutsceneActor::Entity { server_id: NPC });
        assert_eq!(*key, KEY);
        // The hold parks on the renderer's finish report; its deadline is the
        // 60-frame = one-second DAT length.
        assert!(matches!(session.tick(0.5), Advance::Waiting));
        let entry = session.pending_motion_holds.values().next().unwrap();
        assert_eq!(entry.3, std::time::Duration::from_secs(1));
        session.motion_done(CutsceneActor::Entity { server_id: NPC }, KEY);
        assert!(matches!(session.tick(0.6), Advance::Ended { .. }));
    }

    #[test]
    fn zone_scheduler_hold_parks_until_the_renderer_reports_done() {
        const NPC: u32 = 0x010E_6032;
        const EVENT: u16 = 503;
        const ZONE: u16 = 248;
        const KEY: [u8; 4] = *b"mov1";
        // The synthetic root resolves the zone's own model DAT (zone 248's
        // mzb file id) to the one-chunk routine of `frames` length.
        let frames = 60u16;
        let (_dir, root) = motion_dat_root(
            ffxi_dat::zone_dat::zone_id_to_mzb_file_id(ZONE).expect("zone 248"),
            KEY,
            frames,
        );
        let mut program = vec![0x2D];
        program.extend(NPC.to_le_bytes()); // actor1 @1
        program.extend(0u32.to_le_bytes()); // actor2 @5
        program.extend(KEY); // key @9
        program.push(0x54); // WAITMAPSCHEDULOR @13
        program.extend(0u32.to_le_bytes()); // actor1 @14 (retail's guard only)
        program.extend(0u32.to_le_bytes()); // actor2 @18
        program.extend(KEY); // key @22
        program.push(0x21); // END @26
        let block = ffxi_dat::event_dat::EventBlock {
            actor: NPC,
            event_ids: vec![EVENT],
            event_offsets: vec![0],
            references: vec![],
            event_data: program,
        };
        let mut session = DialogSession::new(Some(Arc::new(root)), "Test".into());
        session.loaded_event_zone = Some(ZONE);
        session.loaded_string_zone = Some(ZONE);
        session.event_dat = Some(Arc::new(EventDat {
            blocks: vec![block],
        }));
        session.strings = Some(StringDat::parse(&synth_dat(&[b"test"])).unwrap());
        let trigger = EventTrigger {
            event_zone: ZONE,
            text_zone: ZONE,
            unique_no: NPC,
            act_index: 0,
            event_id: EVENT,
            params: vec![],
            npc_name: None,
        };
        assert!(matches!(session.begin(trigger), Begin::Waiting));
        let cues = session.take_cues();
        let [ResolvedCue::Scene(CutsceneCue::ZoneScheduler { key, actor, .. })] = cues.as_slice()
        else {
            panic!("expected the ZoneScheduler cue: {cues:?}");
        };
        assert_eq!(*key, KEY);
        assert_eq!(*actor, CutsceneActor::Entity { server_id: NPC });
        // The hold parks on the renderer's finish report; its deadline is the
        // 60-frame = one-second DAT length.
        assert!(matches!(session.tick(0.5), Advance::Waiting));
        let entry = session.pending_motion_holds.values().next().unwrap();
        assert_eq!(entry.3, std::time::Duration::from_secs(1));
        session.motion_done(CutsceneActor::Entity { server_id: NPC }, KEY);
        assert!(matches!(session.tick(0.6), Advance::Ended { .. }));
    }

    fn schedulor_session(program: Vec<u8>) -> DialogSession {
        const NPC: u32 = 0x010E_6032;
        const EVENT: u16 = 503;
        const ZONE: u16 = 248;
        let block = ffxi_dat::event_dat::EventBlock {
            actor: NPC,
            event_ids: vec![EVENT],
            event_offsets: vec![0],
            references: vec![],
            event_data: program,
        };
        let mut session = DialogSession::new(None, "Test".into());
        session.loaded_event_zone = Some(ZONE);
        session.loaded_string_zone = Some(ZONE);
        session.event_dat = Some(Arc::new(EventDat {
            blocks: vec![block],
        }));
        session.strings = Some(StringDat::parse(&synth_dat(&[b"test"])).unwrap());
        session
    }

    fn schedulor_trigger() -> EventTrigger {
        const NPC: u32 = 0x010E_6032;
        const EVENT: u16 = 503;
        const ZONE: u16 = 248;
        EventTrigger {
            event_zone: ZONE,
            text_zone: ZONE,
            unique_no: NPC,
            act_index: 0,
            event_id: EVENT,
            params: vec![],
            npc_name: None,
        }
    }

    /// 0x2C names a routine in the actor's model DAT, which the session never
    /// opens: its 0x53 parks on the pending hold until the renderer's finish
    /// report, no matter how much host clock elapses.
    #[test]
    fn schedulor_hold_parks_until_the_renderer_reports_done() {
        const NPC: u32 = 0x010E_6032;
        const KEY: [u8; 4] = *b"kue0";
        let mut program = vec![0x2C];
        program.extend(NPC.to_le_bytes()); // actor1 @1
        program.extend(0u32.to_le_bytes()); // actor2 @5
        program.extend(KEY); // key @9
        program.push(0x53); // WAITSCHEDULOR @13
        program.extend(NPC.to_le_bytes()); // actor1 @14
        program.extend(0u32.to_le_bytes()); // actor2 @18
        program.extend(KEY); // key @22
        program.push(0x21); // END @26
        let mut session = schedulor_session(program);
        assert!(matches!(session.begin(schedulor_trigger()), Begin::Waiting));
        let cues = session.take_cues();
        let [ResolvedCue::Scene(CutsceneCue::ActorMotion { actor, key, .. })] = cues.as_slice()
        else {
            panic!("expected the ActorMotion cue: {cues:?}");
        };
        assert_eq!(*actor, CutsceneActor::Entity { server_id: NPC });
        assert_eq!(*key, KEY);
        // The pending hold has no length: no ticking releases it.
        assert!(matches!(session.tick(3600.0), Advance::Waiting));
        // The renderer's finish report releases it; the 0x53 advances next tick.
        session.motion_done(CutsceneActor::Entity { server_id: NPC }, KEY);
        assert!(matches!(session.tick(0.1), Advance::Ended { .. }));
    }

    #[test]
    fn a_repeated_schedulor_issue_needs_one_report_per_issue() {
        const NPC: u32 = 0x010E_6032;
        const KEY: [u8; 4] = *b"kue0";
        // Two 0x2C issues of the same key, then the 0x53: each 13-byte opcode
        // carries its own actor1 @1 / actor2 @5 / key @9.
        let mut program = Vec::new();
        for op in [0x2C, 0x2C, 0x53] {
            program.push(op);
            program.extend(NPC.to_le_bytes()); // actor1 @1
            program.extend(0u32.to_le_bytes()); // actor2 @5
            program.extend(KEY); // key @9
        }
        program.push(0x21); // END @39
        let mut session = schedulor_session(program);
        assert!(matches!(session.begin(schedulor_trigger()), Begin::Waiting));
        let cues = session.take_cues();
        assert_eq!(cues.len(), 2, "both 0x2C cues drain in one pass");
        let actor = CutsceneActor::Entity { server_id: NPC };
        // The first report leaves the second issue still outstanding.
        session.motion_done(actor, KEY);
        assert!(matches!(session.tick(0.1), Advance::Waiting));
        // The second releases the hold.
        session.motion_done(actor, KEY);
        assert!(matches!(session.tick(0.1), Advance::Ended { .. }));
    }

    #[test]
    fn motion_done_is_a_noop_for_an_unknown_pair() {
        let mut session = DialogSession::new(None, "Test".into());
        // Stray reports (event ended, routine stopped) must not panic.
        session.motion_done(CutsceneActor::Entity { server_id: 1 }, *b"kue0");
        session.motion_done(CutsceneActor::LocalPlayer, *b"kue0");
    }

    /// A stopped routine or a headless session never gets the renderer's
    /// report: the deadline sweep in the tick releases the hold instead.
    #[test]
    fn schedulor_hold_releases_on_its_deadline_without_a_report() {
        const NPC: u32 = 0x010E_6032;
        const KEY: [u8; 4] = *b"kue0";
        let mut program = vec![0x2C];
        program.extend(NPC.to_le_bytes()); // actor1 @1
        program.extend(0u32.to_le_bytes()); // actor2 @5
        program.extend(KEY); // key @9
        program.push(0x53); // WAITSCHEDULOR @13
        program.extend(NPC.to_le_bytes()); // actor1 @14
        program.extend(0u32.to_le_bytes()); // actor2 @18
        program.extend(KEY); // key @22
        program.push(0x21); // END @26
        let mut session = schedulor_session(program);
        assert!(matches!(session.begin(schedulor_trigger()), Begin::Waiting));
        let _ = session.take_cues();
        // Age the entry past its deadline: the renderer never reported.
        let entry = session.pending_motion_holds.values_mut().next().unwrap();
        entry.2 = std::time::Instant::now()
            - (PENDING_MOTION_HOLD_MAX + std::time::Duration::from_secs(1));
        assert!(matches!(session.tick(0.1), Advance::Ended { .. }));
        assert!(session.pending_motion_holds.is_empty());
    }

    #[test]
    fn position_update_resumes_only_after_both_server_acknowledgements() {
        let (mut session, trigger) = position_update_session();
        assert!(matches!(session.begin(trigger), Begin::Waiting));
        let actions = session.take_scene_actions();
        let [ffxi_event::vm::scene::SceneAction::PositionUpdate { position, .. }] =
            actions.as_slice()
        else {
            panic!("{actions:?}")
        };
        session.acknowledge_position(*position);
        assert!(matches!(session.tick(0.2), Advance::Waiting));
        session.acknowledge_event();
        assert!(matches!(
            session.tick(0.2),
            Advance::Ended {
                end_para: 0,
                final_position: Some(final_position)
            } if final_position == *position
        ));
        assert!(session.active_end().is_none());
    }

    #[test]
    fn companion_program_keeps_the_trigger_identity_for_server_replies() {
        let (mut session, trigger) = position_update_session();
        let original = (trigger.unique_no, trigger.act_index, trigger.event_id);
        let dat = Arc::make_mut(session.event_dat.as_mut().unwrap());
        let mut companion = dat.blocks[0].clone();
        companion.actor += 1;
        dat.blocks[0].event_data = vec![0];
        dat.blocks.push(companion);
        assert!(matches!(session.begin(trigger), Begin::Waiting));
        assert_eq!(session.active_end(), Some(original));
        assert_eq!(session.take_scene_actions().len(), 1);
    }

    #[test]
    fn clear_and_cancel_discard_unsent_position_updates() {
        let (mut session, trigger) = position_update_session();
        assert!(matches!(session.begin(trigger), Begin::Waiting));
        session.clear();
        assert!(session.take_scene_actions().is_empty());
        let (mut session, trigger) = position_update_session();
        assert!(matches!(session.begin(trigger), Begin::Waiting));
        assert!(matches!(session.cancel(), Advance::Ended { .. }));
        assert!(session.take_scene_actions().is_empty());
    }

    /// The landmark scan must find the block at its shifted position and
    /// reject a lone duplicate line.
    #[test]
    fn finds_the_fishing_block_by_landmarks() {
        let mut entries: Vec<&[u8]> = vec![b" filler"; 9000];
        // A duplicate of one landmark without the others proves nothing.
        entries[100] = b"You didn't catch anything.";
        let base = 7249usize;
        entries[base + 0x01] = b"You can't fish without a rod in your hands.";
        entries[base + 0x04] = b"You didn't catch anything.";
        entries[base + 0x08] = b"Something caught the hook!";
        let dat = StringDat::parse(&synth_dat(&entries)).expect("parse");
        assert_eq!(find_fishing_block(&dat), Some(base as u16));

        // Without the confirming landmarks there is no block.
        let mut bare: Vec<&[u8]> = vec![b" filler"; 100];
        bare[50] = b"You can't fish without a rod in your hands.";
        let dat = StringDat::parse(&synth_dat(&bare)).expect("parse");
        assert_eq!(find_fishing_block(&dat), None);
    }

    #[test]
    fn substitutes_player_and_speaker_names() {
        let text = "{SpeakerName}: Well met, {PlayerName}.".to_string();
        assert_eq!(
            substitute_names(text, "Zeid", Some("Trion")),
            "Trion: Well met, Zeid."
        );
    }

    #[test]
    fn leaves_speaker_marker_when_name_unknown() {
        let text = "{SpeakerName} greets {PlayerName}.".to_string();
        assert_eq!(
            substitute_names(text, "Zeid", None),
            "{SpeakerName} greets Zeid."
        );
    }

    #[test]
    fn substitutes_text_params_with_the_actor_name() {
        let text = "{ChocoboName:0} caught  {Item:0}!".to_string();
        assert_eq!(
            substitute_text_params(text, Some("Kuluu")),
            "Kuluu caught  {Item:0}!"
        );
        // No name: the marker stays visible rather than vanishing.
        assert_eq!(
            substitute_text_params("{ChocoboName:0} caught!".to_string(), None),
            "{ChocoboName:0} caught!"
        );
    }

    #[test]
    fn substitutes_num_markers_with_params() {
        let text = "You need {Num:0} gil (balance {Num:2}).".to_string();
        assert_eq!(
            substitute_nums(text, &[500, 7, -3]),
            "You need 500 gil (balance -3)."
        );
    }

    #[test]
    fn leaves_num_marker_when_param_missing() {
        let text = "Pay {Num:5} gil.".to_string();
        assert_eq!(substitute_nums(text, &[500]), "Pay {Num:5} gil.");
    }

    #[test]
    fn substitutes_key_item_and_item_markers_with_scraped_names() {
        // Key item 1 = Zeruhn Report (vendor/server/scripts/enum/key_item.lua),
        // item 4509 = Flask of Distilled Water (vendor/server/sql/item_basic.sql).
        let text = "Obtained key item: {KeyItem:0}. Also {Item:1}.".to_string();
        assert_eq!(
            substitute_entity_names(text, &[1, 4509]),
            "Obtained key item: Zeruhn Report. Also Flask of Distilled Water."
        );
    }

    #[test]
    fn leaves_entity_markers_when_unresolvable() {
        let text = "Got {KeyItem:0} and {Item:3}.".to_string();
        assert_eq!(
            substitute_entity_names(text, &[-1]),
            "Got {KeyItem:0} and {Item:3}.",
            "negative id and out-of-range param both stay visible"
        );
    }

    #[test]
    fn frame_params_reach_dialog_nums_and_text() {
        let active = ActiveEvent {
            unique_no: 0x0102,
            act_index: 4,
            event_id: 9,
            agent_event_id: (0x0102u32 << 16) | 9,
            npc_name: Some("Trion".to_string()),
        };
        let frame = ffxi_event::DialogFrame {
            speaker_index: Some(4),
            text: "{SpeakerName}: {Num:1} gil, {PlayerName}.".to_string(),
            choices: vec!["Pay {Num:1}.".to_string(), "Decline.".to_string()],
            params: vec![0, 250],
        };
        let dialog = frame_to_dialog(&active, frame, "Zeid", true);
        assert_eq!(dialog.nums, vec![0, 250]);
        assert_eq!(dialog.prompt.as_deref(), Some("Trion: 250 gil, Zeid."));
        assert_eq!(dialog.choices, vec!["Pay 250.", "Decline."]);
        assert!(dialog.cancel_armed, "the VM's cancel flag rides the frame");
    }

    fn drain(rx: &mut broadcast::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    fn camera_locks(events: &[AgentEvent]) -> Vec<bool> {
        events
            .iter()
            .filter_map(|ev| match ev {
                AgentEvent::CutsceneCue {
                    cue: CutsceneCue::CameraLock { lock },
                } => Some(*lock),
                _ => None,
            })
            .collect()
    }

    /// 0x46 case 1 with no matching case 0 is the norm (1310 of 5269 retail
    /// event bodies), so the scope — not the bytecode — has to give the camera
    /// back, on every way out of the session. Each variant here is a real call
    /// site in `session::keepalive_loop`.
    #[test]
    fn the_camera_lock_is_released_on_every_exit_without_a_case_zero() {
        const EVENT_ID: u32 = 0x010E_602F;
        for exit in [
            EventSessionExit::ScriptEnded,
            EventSessionExit::Cancelled,
            EventSessionExit::WatchdogReleased,
            EventSessionExit::ZoneChanged,
            EventSessionExit::Disconnected,
        ] {
            let (tx, mut rx) = broadcast::channel(16);
            let mut scope = CutsceneScope::default();
            scope.start(EVENT_ID, &tx);
            scope.push(
                ResolvedCue::Scene(CutsceneCue::CameraLock { lock: true }),
                &tx,
            );
            assert!(scope.camera_locked(), "{exit:?}");
            let _ = drain(&mut rx);

            scope.end(exit, &tx);
            assert!(!scope.camera_locked(), "{exit:?} left the camera locked");
            let events = drain(&mut rx);
            assert_eq!(
                camera_locks(&events),
                vec![false],
                "{exit:?} must publish exactly one release: {events:?}"
            );
            assert!(
                matches!(events.last(), Some(AgentEvent::CutsceneEnded)),
                "{exit:?} must close the session last: {events:?}"
            );
            assert!(!scope.is_open(), "{exit:?}");
        }
    }

    /// A second exit on the same session (the watchdog firing behind an
    /// already-sent 0x05B) must not re-announce anything.
    #[test]
    fn ending_an_already_closed_session_is_silent() {
        let (tx, mut rx) = broadcast::channel(16);
        let mut scope = CutsceneScope::default();
        scope.start(1, &tx);
        scope.push(
            ResolvedCue::Scene(CutsceneCue::CameraLock { lock: true }),
            &tx,
        );
        scope.end(EventSessionExit::ScriptEnded, &tx);
        let _ = drain(&mut rx);

        scope.end(EventSessionExit::ZoneChanged, &tx);
        assert!(drain(&mut rx).is_empty());
    }

    /// Case 0 mid-event gives the camera back early; the exit must not send a
    /// second release for a lock nobody holds.
    #[test]
    fn a_mid_event_case_zero_releases_early_and_only_once() {
        let (tx, mut rx) = broadcast::channel(16);
        let mut scope = CutsceneScope::default();
        scope.start(1, &tx);
        for lock in [true, false] {
            scope.push(ResolvedCue::Scene(CutsceneCue::CameraLock { lock }), &tx);
        }
        assert!(!scope.camera_locked());
        scope.end(EventSessionExit::ScriptEnded, &tx);

        let events = drain(&mut rx);
        assert_eq!(camera_locks(&events), vec![true, false], "{events:?}");
    }

    /// 0x5D is a master volume, so it rides the existing music-volume event on
    /// every slot rather than a cue of its own.
    #[test]
    fn music_volume_rides_the_music_event_on_every_slot() {
        const VOLUME: u8 = 40;
        let (tx, mut rx) = broadcast::channel(32);
        let mut scope = CutsceneScope::default();
        scope.start(1, &tx);
        scope.push(
            ResolvedCue::MusicVolume {
                volume: VOLUME,
                fade_frames: 30,
            },
            &tx,
        );
        let slots: Vec<u8> = drain(&mut rx)
            .into_iter()
            .filter_map(|ev| match ev {
                AgentEvent::MusicVolumeChanged { slot, volume } => {
                    assert_eq!(volume, VOLUME);
                    Some(slot)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            slots,
            (0..crate::state::MUSIC_SLOT_COUNT).collect::<Vec<_>>()
        );
    }

    /// The VM leaves its actor operands unresolved on purpose: the local
    /// player, the event's own entity and a named entity all have to stay
    /// distinguishable here (hiding the posed NPC vs mounting the player).
    #[test]
    fn actor_lookups_resolve_against_the_running_events_entity() {
        const EVENT_ENTITY: u32 = 0x010E_602F;
        const POSED_NPC: u32 = 0x010E_6032;

        let hide = |lookup| {
            resolve_cue(
                EventCue::ActorHide {
                    target: lookup,
                    hide: true,
                },
                EVENT_ENTITY,
                0,
            )
        };
        let target = |cue| match cue {
            ResolvedCue::Scene(CutsceneCue::ActorHide { target, .. }) => target,
            other => panic!("not a hide cue: {other:?}"),
        };

        assert_eq!(
            target(hide(ActorLookup::LOCAL_PLAYER)),
            CutsceneActor::LocalPlayer
        );
        assert_eq!(
            target(hide(ActorLookup::EVENT_ENTITY)),
            CutsceneActor::Entity {
                server_id: EVENT_ENTITY
            }
        );
        assert_eq!(
            target(hide(ActorLookup(POSED_NPC))),
            CutsceneActor::Entity {
                server_id: POSED_NPC
            }
        );
    }

    /// 0xB5 names the event entity by default: the rename cue rides the
    /// running event's own server id.
    #[test]
    fn entity_name_cue_resolves_to_the_running_events_entity() {
        const EVENT_ENTITY: u32 = 0x010E_602F;
        let name: [u8; 16] = *b"Sajj'aka\0\0\0\0\0\0\0\0";
        let cue = resolve_cue(
            EventCue::EntityName {
                actor: ActorLookup::EVENT_ENTITY,
                name,
            },
            EVENT_ENTITY,
            0,
        );
        match cue {
            ResolvedCue::Scene(CutsceneCue::EntityName { actor, name: got }) => {
                assert_eq!(
                    actor,
                    CutsceneActor::Entity {
                        server_id: EVENT_ENTITY
                    }
                );
                assert_eq!(got, name);
            }
            other => panic!("not a name cue: {other:?}"),
        }
    }

    /// PENDINGSTR can land before the event's VM exists: it is accepted and
    /// dropped, not an error.
    #[test]
    fn apply_pending_str_without_a_runner_is_a_noop() {
        let mut session = DialogSession::new(None, "tester".into());
        session.apply_pending_str(&[[0u8; 16]; 4]);
        assert!(session.runner.is_none());
    }
}
