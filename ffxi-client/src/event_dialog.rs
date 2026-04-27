//! Session-side bridge from the event-trigger packets (0x32) to the event VM.
//!
//! Holds the per-zone event + dialog DATs and the active [`DialogRunner`] across
//! player interactions, turning VM yields into real [`DialogState`]s. When no
//! event DAT can drive a trigger, [`DialogSession::begin`] returns `None` and the
//! caller falls back to the legacy raw-packet dialog.

use std::sync::Arc;

use ffxi_dat::dmsg::StringDat;
use ffxi_dat::event_dat::EventDat;
use ffxi_dat::DatRoot;
use ffxi_event::{DialogRunner, DialogStep};

use crate::state::DialogState;

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
    /// The event is over — the caller sends EVENT_END.
    Ended,
}

pub struct DialogSession {
    dat_root: Option<Arc<DatRoot>>,
    loaded_zone: Option<u16>,
    event_dat: Option<EventDat>,
    strings: Option<StringDat>,
    runner: Option<DialogRunner>,
    active: Option<ActiveEvent>,
}

impl DialogSession {
    pub fn new(dat_root: Option<Arc<DatRoot>>) -> Self {
        Self {
            dat_root,
            loaded_zone: None,
            event_dat: None,
            strings: None,
            runner: None,
            active: None,
        }
    }

    /// `(unique_no, act_index, event_id)` of the active VM-driven event, for the
    /// EVENT_END reply. `None` when no VM event is running (legacy path).
    pub fn active_end(&self) -> Option<(u32, u16, u16)> {
        self.active
            .as_ref()
            .map(|a| (a.unique_no, a.act_index, a.event_id))
    }

    fn ensure_zone(&mut self, zone: u16) {
        if self.loaded_zone == Some(zone) {
            return;
        }
        self.loaded_zone = Some(zone);
        self.event_dat = load_event_dat(self.dat_root.as_deref(), zone);
        self.strings = load_strings(self.dat_root.as_deref(), zone);
    }

    /// Begin a VM-driven event for a 0x32 trigger. Returns the first frame as a
    /// [`DialogState`], or `None` if the event can't be driven (no DAT, no such
    /// block/event, or the VM stalls immediately) so the caller uses the legacy
    /// raw dialog instead.
    pub fn begin(
        &mut self,
        zone: u16,
        unique_no: u32,
        act_index: u16,
        event_id: u16,
        npc_name: Option<String>,
    ) -> Option<DialogState> {
        self.ensure_zone(zone);
        let strings = self.strings.as_ref()?;
        let block = self.event_dat.as_ref()?.block_for_actor(unique_no)?;
        let mut runner = DialogRunner::start(block, event_id, act_index)?;
        let step = runner.advance(None, strings);
        let active = ActiveEvent {
            unique_no,
            act_index,
            event_id,
            agent_event_id: ((unique_no as u64) << 16 | event_id as u64) as u32,
            npc_name,
        };
        match step {
            DialogStep::Frame(frame) => {
                let dialog = frame_to_dialog(&active, frame);
                self.runner = Some(runner);
                self.active = Some(active);
                Some(dialog)
            }
            DialogStep::Ended | DialogStep::Stopped(_) => {
                self.clear();
                None
            }
        }
    }

    /// Apply the player's response (dismiss, or `Some(index)` choice) and return
    /// the next frame or [`Advance::Ended`]. Call only while [`active_end`] is
    /// `Some`.
    ///
    /// [`active_end`]: Self::active_end
    pub fn advance(&mut self, choice: Option<u32>) -> Advance {
        let (Some(strings), Some(runner), Some(active)) = (
            self.strings.as_ref(),
            self.runner.as_mut(),
            self.active.as_ref(),
        ) else {
            self.clear();
            return Advance::Ended;
        };
        match runner.advance(choice, strings) {
            DialogStep::Frame(frame) => {
                let dialog = frame_to_dialog(active, frame);
                Advance::Frame(dialog)
            }
            DialogStep::Ended | DialogStep::Stopped(_) => {
                self.clear();
                Advance::Ended
            }
        }
    }

    pub fn clear(&mut self) {
        self.runner = None;
        self.active = None;
    }
}

fn frame_to_dialog(active: &ActiveEvent, frame: ffxi_event::DialogFrame) -> DialogState {
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
        nums: Vec::new(),
        prompt: Some(frame.text),
        choices: frame.choices,
    }
}

fn load_event_dat(root: Option<&DatRoot>, zone: u16) -> Option<EventDat> {
    let root = root?;
    let loc = ffxi_dat::event_locate::zone_id_to_event_location(zone)?;
    let bytes = std::fs::read(loc.path_under(root.root())).ok()?;
    EventDat::parse(&bytes).ok()
}

fn load_strings(root: Option<&DatRoot>, zone: u16) -> Option<StringDat> {
    let root = root?;
    let file_id = ffxi_dat::zone_dat::zone_id_to_string_file_id(zone)?;
    let loc = root.resolve(file_id).ok()?;
    let bytes = std::fs::read(loc.path_under(root.root())).ok()?;
    StringDat::parse(&bytes).ok()
}
