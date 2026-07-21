//! Session-side bridge from the event-trigger packets (0x32) to the event VM.
//!
//! Holds the per-zone event + dialog DATs and the active [`DialogRunner`] across
//! player interactions, turning VM yields into real [`DialogState`]s. When no
//! event DAT can drive a trigger, [`DialogSession::begin`] returns
//! [`Begin::Undriveable`] and the caller auto-releases the event (EVENT_END)
//! rather than pin the character InEvent behind an empty dialog.

use std::sync::Arc;

use ffxi_dat::dmsg::{
    plain_marker, StringDat, MARKER_ITEM, MARKER_KEY_ITEM, MARKER_NUM, MARKER_PLAYER_NAME,
    MARKER_SPEAKER_NAME,
};
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
        // A 0x32 trigger carries no numeric parameters (`num[8]` arrives only
        // on the 0x33/0x34 triggers), so the VM runs with an empty params
        // array and any `{Num:N}` markers stay unresolved in the text.
        let Some(mut runner) = DialogRunner::start(block, event_id, act_index, Vec::new()) else {
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
        self.drive(|runner, strings| runner.advance(choice, strings))
    }

    /// Cancel the in-progress event from any frame (the Esc path): the VM
    /// reports the frame's cancel result and ends with
    /// [`ffxi_event::EVENT_CANCELLED_END_PARA`].
    pub fn cancel(&mut self) -> Advance {
        self.drive(|runner, strings| runner.cancel(strings))
    }

    fn drive(&mut self, step: impl FnOnce(&mut DialogRunner, &StringDat) -> DialogStep) -> Advance {
        let (Some(strings), Some(runner), Some(active)) = (
            self.strings.as_ref(),
            self.runner.as_mut(),
            self.active.as_ref(),
        ) else {
            self.clear();
            return Advance::Ended { end_para: 0 };
        };
        match step(runner, strings) {
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

    /// Entry `index` of `zone`'s dialog DAT, loading it if needed. `None` when
    /// the zone has no available string DAT (missing FFXI_DAT_PATH, unmapped
    /// zone) or the index is out of range.
    pub fn zone_text(&mut self, zone: u16, index: usize) -> Option<String> {
        self.ensure_zone(zone);
        self.strings.as_ref()?.text(index)
    }
}

fn frame_to_dialog(
    active: &ActiveEvent,
    frame: ffxi_event::DialogFrame,
    player_name: &str,
) -> DialogState {
    let ffxi_event::DialogFrame {
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
        ffxi_proto::key_item_names::lookup(id).map(str::to_string)
    });
    substitute_param_marker(text, MARKER_ITEM, &|index| {
        let id = u16::try_from(*params.get(index)?).ok()?;
        ffxi_proto::item_names::lookup(id).map(str::to_string)
    })
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

// The load failures below are logged once per zone: `ensure_zone` only calls
// these when `loaded_zone` changes, and caches the (None) result (kuluu-zkuf).

fn load_event_dat(root: Option<&DatRoot>, zone: u16) -> Option<EventDat> {
    let root = root?;
    let Some(loc) = ffxi_dat::event_locate::zone_id_to_event_location(zone) else {
        tracing::warn!(
            zone,
            "no event DAT mapping for zone; NPC dialog disabled for this zone"
        );
        return None;
    };
    let path = loc.path_under(root.root());
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

fn load_strings(root: Option<&DatRoot>, zone: u16) -> Option<StringDat> {
    let root = root?;
    let Some(file_id) = ffxi_dat::zone_dat::zone_id_to_string_file_id(zone) else {
        tracing::warn!(
            zone,
            "no string DAT mapping for zone; NPC dialog disabled for this zone"
        );
        return None;
    };
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
    let path = loc.path_under(root.root());
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
            speaker_index: 4,
            text: "{SpeakerName}: {Num:1} gil, {PlayerName}.".to_string(),
            choices: vec!["Pay {Num:1}.".to_string(), "Decline.".to_string()],
            params: vec![0, 250],
        };
        let dialog = frame_to_dialog(&active, frame, "Zeid");
        assert_eq!(dialog.nums, vec![0, 250]);
        assert_eq!(dialog.prompt.as_deref(), Some("Trion: 250 gil, Zeid."));
        assert_eq!(dialog.choices, vec!["Pay 250.", "Decline."]);
    }
}
