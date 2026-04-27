//! Drives an [`EventVm`] against a zone's dialog strings to produce renderable
//! dialog frames — the bridge the session holds across player interactions.

use ffxi_dat::dmsg::StringDat;
use ffxi_dat::event_dat::EventBlock;

use crate::vm::{EventVm, StepResult};

/// One renderable dialog frame: NPC speech (and, for a menu, the selectable
/// `choices`). `text` is already decoded from the dialog DAT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogFrame {
    pub speaker_index: u16,
    pub text: String,
    pub choices: Vec<String>,
}

/// Result of advancing the dialog one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogStep {
    /// Show this frame and wait for the player; pass their response to the next
    /// [`DialogRunner::advance`].
    Frame(DialogFrame),
    /// The event finished (or cancelled) — the session sends EVENT_END.
    Ended,
    /// Hit an opcode the VM can't run; the session falls back (EVENT_END) rather
    /// than render a wrong frame. `op` is the opcode value.
    Stopped(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Start,
    Message,
    Choice,
}

/// Long-lived per-event driver. The `StringDat` is passed to [`advance`] rather
/// than owned so the session can keep one zone string table cached and shared.
///
/// [`advance`]: Self::advance
pub struct DialogRunner {
    vm: EventVm,
    pending: Pending,
}

impl DialogRunner {
    /// Start `event_id` from `block` with `speaker_index` (the NPC's target
    /// index). `None` if the block has no such event.
    pub fn start(block: &EventBlock, event_id: u16, speaker_index: u16) -> Option<Self> {
        Some(Self {
            vm: EventVm::start(block, event_id, speaker_index)?,
            pending: Pending::Start,
        })
    }

    /// Apply the player's response to the previous frame and run to the next one.
    /// `choice` is the selected option index for a menu frame (`None` cancels);
    /// it is ignored for a message frame and on the first call.
    pub fn advance(&mut self, choice: Option<u32>, strings: &StringDat) -> DialogStep {
        match self.pending {
            Pending::Message => self.vm.dismiss_message(),
            Pending::Choice => self.vm.select_choice(choice),
            Pending::Start => {}
        }
        loop {
            match self.vm.step() {
                StepResult::AwaitMessage(m) => {
                    self.pending = Pending::Message;
                    return DialogStep::Frame(DialogFrame {
                        speaker_index: m.speaker_index,
                        text: message_text(strings, m.message_id),
                        choices: Vec::new(),
                    });
                }
                StepResult::AwaitMessageAck => self.vm.dismiss_message(),
                StepResult::AwaitChoice(c) => {
                    self.pending = Pending::Choice;
                    let (text, choices) = choice_text(strings, c.message_id);
                    return DialogStep::Frame(DialogFrame {
                        speaker_index: c.speaker_index,
                        text,
                        choices,
                    });
                }
                StepResult::Done | StepResult::Cancelled => return DialogStep::Ended,
                StepResult::Unimplemented(op) => return DialogStep::Stopped(op),
            }
        }
    }
}

fn message_text(strings: &StringDat, message_id: u32) -> String {
    clean_display(&strings.text(message_id as usize).unwrap_or_default())
}

/// Split a menu entry into its prompt and selectable options via the faithful
/// Selection marker (`StringDat::menu`); falls back to the first-line-is-prompt
/// heuristic for entries that lack it.
fn choice_text(strings: &StringDat, message_id: u32) -> (String, Vec<String>) {
    if let Some((prompt, options)) = strings.menu(message_id as usize) {
        let options: Vec<String> = options
            .iter()
            .map(|o| clean_display(o))
            .filter(|o| !o.is_empty())
            .collect();
        return (clean_display(&prompt), options);
    }
    let raw = clean_display(&strings.text(message_id as usize).unwrap_or_default());
    let mut lines = raw.split('\n').filter(|l| !l.trim().is_empty());
    let prompt = lines.next().unwrap_or_default().to_string();
    let choices: Vec<String> = lines.map(str::to_string).collect();
    (prompt, choices)
}

/// Strip the auto-translate / layout `{Auto:N}` markers the dmsg decoder emits;
/// they are formatting terminators, not visible text. Substitution placeholders
/// (`{PlayerName}`, `{Num:N}`, `{Choice:N}`, …) are left for later resolution.
fn clean_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("{Auto") {
        out.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(end) => rest = &rest[start + end + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::event_dat::{EventDat, ZONE_PLAYER_ACTOR};
    use ffxi_dat::DatRoot;
    use std::path::Path;

    #[test]
    fn clean_display_strips_auto_markers_but_keeps_substitutions() {
        assert_eq!(clean_display("Nothing.{Auto:49}"), "Nothing.");
        assert_eq!(clean_display("a{Auto:1}b{Auto:2}c"), "abc");
        assert_eq!(
            clean_display("Good luck, {Choice:0}!"),
            "Good luck, {Choice:0}!"
        );
    }

    fn install() -> Option<DatRoot> {
        if let Ok(r) = DatRoot::from_env() {
            return Some(r);
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("vendor/game-files/SquareEnix/FINAL FANTASY XI");
        dir.join("VTABLE.DAT")
            .exists()
            .then(|| DatRoot::open(dir).ok())
            .flatten()
    }

    /// Run real event bytecode from the install through the VM + dialog DAT and
    /// confirm the pipeline drives without panicking, reporting how far the
    /// implemented opcode subset gets. Self-skips without an install.
    #[test]
    fn drives_real_zone_events() {
        let Some(root) = install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };

        // Walk a handful of early zones; for each, run the zone/player block's
        // events (and the first NPC block's) and tally outcomes.
        let mut frames = 0usize;
        let mut ended = 0usize;
        let mut stopped: std::collections::BTreeMap<u8, usize> = Default::default();

        for zone in 1u16..60 {
            let Some(eloc) = ffxi_dat::event_locate::zone_id_to_event_location(zone) else {
                continue;
            };
            let Ok(ebytes) = std::fs::read(eloc.path_under(root.root())) else {
                continue;
            };
            let Ok(edat) = EventDat::parse(&ebytes) else {
                continue;
            };
            let Some(sfid) = ffxi_dat::zone_dat::zone_id_to_string_file_id(zone) else {
                continue;
            };
            let Ok(sloc) = root.resolve(sfid) else {
                continue;
            };
            let Ok(sbytes) = std::fs::read(sloc.path_under(root.root())) else {
                continue;
            };
            let Ok(strings) = StringDat::parse(&sbytes) else {
                continue;
            };

            for block in edat
                .blocks
                .iter()
                .filter(|b| b.actor != ZONE_PLAYER_ACTOR)
                .take(2)
            {
                for &eid in block.event_ids.iter().take(2) {
                    let Some(mut runner) = DialogRunner::start(block, eid, 0) else {
                        continue;
                    };
                    // Bound the interaction loop; auto-pick option 0 for menus.
                    for _ in 0..16 {
                        match runner.advance(Some(0), &strings) {
                            DialogStep::Frame(_) => frames += 1,
                            DialogStep::Ended => {
                                ended += 1;
                                break;
                            }
                            DialogStep::Stopped(op) => {
                                *stopped.entry(op).or_default() += 1;
                                break;
                            }
                        }
                    }
                }
            }
        }

        eprintln!(
            "real-event drive: {frames} frames, {ended} ended cleanly, stopped on opcodes {stopped:?}"
        );
        // The pipeline must at least produce real dialog frames from real bytecode.
        assert!(frames > 0, "no dialog frames produced from real event DATs");
    }

    /// Regression for the trigger-field + opcode fixes: a known live talk NPC —
    /// Harara, W.W. in Windurst Woods (zone 241, server id 0x010F10BF, a Conquest
    /// Overseer whose talk event is `EventPara` = 32759, not the zone-valued
    /// `EventNum`). The VM must drive event 32759 to real, non-empty dialog text
    /// and end cleanly (no `Stopped`). Self-skips without an install.
    #[test]
    fn drives_harara_windurst_woods() {
        let Some(root) = install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        const ZONE: u16 = 241;
        const HARARA: u32 = 17_764_543; // 0x010F10BF
        const TALK_EVENT: u16 = 32759; // guardEvent (Harara_WW.lua), sent as EventPara
        const ACT_INDEX: u16 = 0xBF;

        let eloc = ffxi_dat::event_locate::zone_id_to_event_location(ZONE).expect("event loc");
        let ebytes = std::fs::read(eloc.path_under(root.root())).expect("read event dat");
        let edat = EventDat::parse(&ebytes).expect("parse event dat");
        let sfid = ffxi_dat::zone_dat::zone_id_to_string_file_id(ZONE).expect("string file id");
        let sloc = root.resolve(sfid).expect("resolve string dat");
        let sbytes = std::fs::read(sloc.path_under(root.root())).expect("read string dat");
        let strings = StringDat::parse(&sbytes).expect("parse string dat");

        let block = edat
            .block_for_actor(HARARA)
            .unwrap_or_else(|| panic!("no event block for Harara 0x{HARARA:08X}"));
        let mut runner =
            DialogRunner::start(block, TALK_EVENT, ACT_INDEX).expect("Harara has talk event 32759");

        let mut frames = Vec::new();
        let mut ended = false;
        for _ in 0..16 {
            match runner.advance(Some(0), &strings) {
                DialogStep::Frame(f) => frames.push(f.text),
                DialogStep::Ended => {
                    ended = true;
                    break;
                }
                DialogStep::Stopped(op) => panic!("event 32759 stopped on opcode 0x{op:02X}"),
            }
        }
        assert!(ended, "event 32759 did not end cleanly within 16 steps");
        assert!(
            frames.iter().any(|f| !f.trim().is_empty()),
            "event 32759 produced no real dialog text: {frames:?}"
        );
    }
}
