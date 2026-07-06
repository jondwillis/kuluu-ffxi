//! Session-side bridge from the event-trigger packets (0x32) to the event VM.
//!
//! Holds the per-zone event + dialog DATs and the active [`DialogRunner`] across
//! player interactions, turning VM yields into real [`DialogState`]s. When no
//! event DAT can drive a trigger, [`DialogSession::begin`] returns
//! [`Begin::Undriveable`] and the caller auto-releases the event (EVENT_END)
//! rather than pin the character InEvent behind an empty dialog.

use std::sync::Arc;

use ffxi_dat::dmsg::{plain_marker, StringDat, MARKER_PLAYER_NAME, MARKER_SPEAKER_NAME};
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
    /// The event is over — the caller sends EVENT_END with `end_para` as the
    /// 0x05B `EndPara` (the VM's `Work_Zone[1]`, or a cancel sentinel).
    Ended { end_para: u32 },
}

/// Outcome of starting a VM-driven event.
pub enum Begin {
    /// Show the first frame and wait for the player.
    Frame(DialogState),
    /// The VM ran the whole event without producing a dialog frame
    /// (choreography-only or bookkeeping script) — the caller sends EVENT_END
    /// with `end_para`, same as [`Advance::Ended`].
    Ended { end_para: u32 },
    /// The VM can't drive the event: missing DAT/strings/block, or it stopped
    /// on unimplemented opcode `stopped_op`.
    Undriveable { stopped_op: Option<u8> },
}

pub struct DialogSession {
    dat_root: Option<Arc<DatRoot>>,
    /// Logged-in character name, substituted for the `{PlayerName}` dialog marker.
    player_name: String,
    loaded_zone: Option<u16>,
    event_dat: Option<EventDat>,
    strings: Option<StringDat>,
    runner: Option<DialogRunner>,
    active: Option<ActiveEvent>,
}

impl DialogSession {
    pub fn new(dat_root: Option<Arc<DatRoot>>, player_name: String) -> Self {
        Self {
            dat_root,
            player_name,
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

    /// Begin a VM-driven event for a 0x32 trigger.
    pub fn begin(
        &mut self,
        zone: u16,
        unique_no: u32,
        act_index: u16,
        event_id: u16,
        npc_name: Option<String>,
    ) -> Begin {
        self.ensure_zone(zone);
        let undriveable = Begin::Undriveable { stopped_op: None };
        let Some(strings) = self.strings.as_ref() else {
            return undriveable;
        };
        let Some(block) = self
            .event_dat
            .as_ref()
            .and_then(|dat| dat.block_for_actor(unique_no))
        else {
            return undriveable;
        };
        let Some(mut runner) = DialogRunner::start(block, event_id, act_index) else {
            return undriveable;
        };
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
                let dialog = frame_to_dialog(&active, frame, &self.player_name);
                self.runner = Some(runner);
                self.active = Some(active);
                Begin::Frame(dialog)
            }
            DialogStep::Ended { end_para } => {
                self.clear();
                Begin::Ended { end_para }
            }
            DialogStep::Stopped(op) => {
                self.clear();
                Begin::Undriveable {
                    stopped_op: Some(op),
                }
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
            return Advance::Ended { end_para: 0 };
        };
        match runner.advance(choice, strings) {
            DialogStep::Frame(frame) => {
                let dialog = frame_to_dialog(active, frame, &self.player_name);
                Advance::Frame(dialog)
            }
            DialogStep::Ended { end_para } => {
                self.clear();
                Advance::Ended { end_para }
            }
            DialogStep::Stopped(op) => {
                tracing::warn!(
                    op = format!("0x{op:02X}"),
                    "event VM stopped mid-dialog; releasing with end_para 0"
                );
                self.clear();
                Advance::Ended { end_para: 0 }
            }
        }
    }

    pub fn clear(&mut self) {
        self.runner = None;
        self.active = None;
    }
}

fn frame_to_dialog(
    active: &ActiveEvent,
    frame: ffxi_event::DialogFrame,
    player_name: &str,
) -> DialogState {
    let substitute = |text: String| substitute_names(text, player_name, active.npc_name.as_deref());
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
        prompt: Some(substitute(frame.text)),
        choices: frame.choices.into_iter().map(substitute).collect(),
    }
}

/// Resolve the plain name markers the dmsg decoder leaves in dialog text:
/// `{PlayerName}` → the logged-in character, `{SpeakerName}` → the speaking NPC.
/// A `{SpeakerName}` with no known speaker name is left as-is.
fn substitute_names(text: String, player_name: &str, speaker_name: Option<&str>) -> String {
    let text = text.replace(&plain_marker(MARKER_PLAYER_NAME), player_name);
    match speaker_name {
        Some(name) => text.replace(&plain_marker(MARKER_SPEAKER_NAME), name),
        None => text,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
