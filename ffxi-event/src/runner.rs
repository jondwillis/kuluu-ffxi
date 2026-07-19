//! Drives an [`EventVm`] against a zone's dialog strings to produce renderable
//! dialog frames — the bridge the session holds across player interactions.

use ffxi_dat::dmsg::{
    StringDat, AUTO_MARKER_PREFIX, CHOICE_MARKER_PREFIX, SET_COLOR_MARKER_PREFIX,
};
use ffxi_dat::event_dat::EventBlock;

use crate::vm::{EventVm, StepResult};

/// 0x05B `EndPara` the client returns for a cancelled event in place of
/// `Work_Zone[1]` (research/XiPackets/world/client/0x005B); LSB scripts match
/// it as `utils.EVENT_CANCELLED_OPTION` (vendor/server/scripts/utils/utils.lua:8).
pub const EVENT_CANCELLED_END_PARA: u32 = 1 << 30;

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
    /// The event finished — the session sends EVENT_END with `end_para`, the
    /// value the client returns in the 0x05B `EndPara`: `Work_Zone[1]` for a
    /// normal end, [`EVENT_CANCELLED_END_PARA`] for a cancel
    /// (research/XiPackets/world/client/0x005B).
    Ended { end_para: u32 },
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
        self.run(strings)
    }

    /// Cancel out of the current frame (the Esc path): a menu reports the
    /// cancel selection, a message invalidates the open dialog; either way the
    /// VM ends the event with [`EVENT_CANCELLED_END_PARA`].
    pub fn cancel(&mut self, strings: &StringDat) -> DialogStep {
        match self.pending {
            Pending::Message | Pending::Start => self.vm.cancel_message(),
            Pending::Choice => self.vm.select_choice(None),
        }
        self.run(strings)
    }

    fn run(&mut self, strings: &StringDat) -> DialogStep {
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
                StepResult::Done => {
                    return DialogStep::Ended {
                        end_para: self.vm.work_zone(1) as u32,
                    }
                }
                StepResult::Cancelled => {
                    return DialogStep::Ended {
                        end_para: EVENT_CANCELLED_END_PARA,
                    }
                }
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

/// Strip the formatting markers the dmsg decoder emits (`{Auto:N}` layout
/// terminators, `{SetColor:N}` text-color codes) and resolve `{Choice:N}[a/b/…]`
/// alternatives (see [`resolve_choice_brackets`]). The remaining substitution
/// placeholders (`{PlayerName}`, `{SpeakerName}`, `{Num:N}`, …) are left for the
/// caller, which has the runtime names/parameters they need.
fn clean_display(s: &str) -> String {
    let stripped = strip_marker(s, AUTO_MARKER_PREFIX);
    let stripped = strip_marker(&stripped, SET_COLOR_MARKER_PREFIX);
    resolve_choice_brackets(&stripped).trim().to_string()
}

/// Remove every `prefix…}` run — a formatting marker with no visible text.
fn strip_marker(s: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(prefix) {
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
    out
}

/// Alternative picked for a `{Choice:N}[a/b/…]` run when the selecting message
/// parameter isn't available. Retail reads message parameter `N` and shows that
/// alternative; the parameter array isn't threaded through the VM yet (kuluu-6zy),
/// so we take the first alternative, which is correct for the common case where a
/// nation/gender variant simply lists its default first.
const UNRESOLVED_CHOICE_ALT: usize = 0;

/// Collapse `{Choice:N}[opt0/opt1/…]` runs to a single alternative — the dmsg
/// decoder emits the `{Choice:N}` marker from control code 0x0C and leaves the
/// following `[a/b]` bracket as literal text. `N` selects the alternative via a
/// message parameter; lacking it we take [`UNRESOLVED_CHOICE_ALT`]. A `{Choice:N}`
/// with no immediately-following bracket is left verbatim.
fn resolve_choice_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find(CHOICE_MARKER_PREFIX) {
        let after_tag = &rest[pos + CHOICE_MARKER_PREFIX.len()..];
        let Some(close) = after_tag.find('}') else {
            break;
        };
        let tail = &after_tag[close + 1..];
        match tail
            .strip_prefix('[')
            .and_then(|b| b.find(']').map(|e| (b, e)))
        {
            Some((inner, end)) => {
                out.push_str(&rest[..pos]);
                let alts: Vec<&str> = inner[..end].split('/').collect();
                let chosen = alts
                    .get(UNRESOLVED_CHOICE_ALT)
                    .or_else(|| alts.first())
                    .copied()
                    .unwrap_or("");
                out.push_str(chosen);
                rest = &inner[end + 1..];
            }
            None => {
                let consumed = pos + CHOICE_MARKER_PREFIX.len() + close + 1;
                out.push_str(&rest[..consumed]);
                rest = &rest[consumed..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::event_dat::{EventDat, ZONE_PLAYER_ACTOR};
    use ffxi_dat::DatRoot;
    use std::path::Path;

    #[test]
    fn clean_display_strips_formatting_but_keeps_substitutions() {
        assert_eq!(clean_display("Nothing.{Auto:49}"), "Nothing.");
        assert_eq!(clean_display("a{Auto:1}b{Auto:2}c"), "abc");
        // {SetColor:N} is a text-color code, not visible text.
        assert_eq!(clean_display("red{SetColor:5}text"), "redtext");
        // Name substitutions are the caller's job (runtime names); left intact.
        assert_eq!(
            clean_display("Hello, {PlayerName}."),
            "Hello, {PlayerName}."
        );
        // A {Choice:N} with no following bracket has nothing to resolve.
        assert_eq!(
            clean_display("Good luck, {Choice:0}!"),
            "Good luck, {Choice:0}!"
        );
    }

    #[test]
    fn clean_display_resolves_choice_alternatives() {
        assert_eq!(
            clean_display("Good luck, {Choice:0}[citizen/comrade]. See you."),
            "Good luck, citizen. See you."
        );
    }

    #[test]
    fn resolve_choice_brackets_takes_first_alternative() {
        // The baked N is the parameter index, not the alternative; without the
        // parameter we always take the first alternative.
        assert_eq!(resolve_choice_brackets("a {Choice:3}[x/y/z] b"), "a x b");
        assert_eq!(resolve_choice_brackets("{Choice:0}[only]"), "only");
    }

    #[test]
    fn resolve_choice_brackets_handles_multiple_and_bare_markers() {
        assert_eq!(
            resolve_choice_brackets("{Choice:0}[he/she] told {Choice:0}[him/her]"),
            "he told him"
        );
        // No bracket -> marker left verbatim.
        assert_eq!(
            resolve_choice_brackets("plain {Choice:1} end"),
            "plain {Choice:1} end"
        );
    }

    use crate::vm::{OP_END, OP_MESSAGE, OP_MESWAIT, OP_QUERY, OP_QUERYWAIT};

    fn one_event_block(
        event_data: Vec<u8>,
        references: Vec<u32>,
    ) -> ffxi_dat::event_dat::EventBlock {
        ffxi_dat::event_dat::EventBlock {
            actor: ZONE_PLAYER_ACTOR,
            event_ids: vec![1],
            event_offsets: vec![0],
            references,
            event_data,
        }
    }

    /// Minimal valid DialogTable with a single zero-length entry, in the dmsg
    /// header layout ([`StringDat::parse`] validating it pins the format —
    /// magic = 0x1000_0000 + data_len, offsets XOR 0x8080_8080).
    fn empty_strings() -> StringDat {
        let data_len = 4u32;
        let mut buf = Vec::new();
        buf.extend_from_slice(&(0x1000_0000u32 + data_len).to_le_bytes());
        buf.extend_from_slice(&(4u32 ^ 0x8080_8080).to_le_bytes());
        StringDat::parse(&buf).expect("synthetic DialogTable")
    }

    /// Regression: Esc on a message frame must genuinely cancel — advance would
    /// dismiss and surface the second message frame (Esc == Enter, kuluu-76z).
    #[test]
    fn cancel_on_message_frame_ends_cancelled_not_next_frame() {
        let data = vec![
            OP_MESSAGE, 0x00, 0x80, OP_MESWAIT, OP_MESSAGE, 0x01, 0x80, OP_MESWAIT, OP_END,
        ];
        let strings = empty_strings();
        let mut r = DialogRunner::start(&one_event_block(data, vec![10, 11]), 1, 0).unwrap();
        assert!(matches!(r.advance(None, &strings), DialogStep::Frame(_)));
        assert_eq!(
            r.cancel(&strings),
            DialogStep::Ended {
                end_para: EVENT_CANCELLED_END_PARA,
            }
        );
    }

    #[test]
    fn cancel_on_choice_frame_ends_cancelled() {
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
        let strings = empty_strings();
        let mut r = DialogRunner::start(&one_event_block(data, vec![500, 0]), 1, 0).unwrap();
        assert!(matches!(r.advance(None, &strings), DialogStep::Frame(_)));
        assert_eq!(
            r.cancel(&strings),
            DialogStep::Ended {
                end_para: EVENT_CANCELLED_END_PARA,
            }
        );
    }

    /// Guard: the cancel sentinel is the exact value LSB scripts branch on
    /// (utils.EVENT_CANCELLED_OPTION = bit.lshift(1, 30),
    /// vendor/server/scripts/utils/utils.lua:8).
    #[test]
    fn cancel_sentinel_is_lsb_event_cancelled_option() {
        assert_eq!(EVENT_CANCELLED_END_PARA, 0x4000_0000);
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
                            DialogStep::Ended { .. } => {
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
                DialogStep::Ended { .. } => {
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
        assert!(
            frames.iter().all(|f| !f.contains(CHOICE_MARKER_PREFIX)),
            "unresolved {{Choice:N}} marker leaked into Harara's dialog: {frames:?}"
        );
    }

    /// Picking the first option of Harara's menu ("Would you cast Signet on me?")
    /// must end the event with `end_para == 1` — the `Work_Zone[1]` the client
    /// returns in the 0x05B `EndPara`, which is the exact value
    /// vendor/server/scripts/globals/conquest.lua (overseerOnEventFinish:
    /// `if option == 1`) requires to grant Signet. Self-skips without an install.
    #[test]
    fn harara_signet_pick_returns_option_1() {
        let Some(root) = install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        const ZONE: u16 = 241;
        const HARARA: u32 = 17_764_543;
        const TALK_EVENT: u16 = 32759;
        const ACT_INDEX: u16 = 0xBF;

        let eloc = ffxi_dat::event_locate::zone_id_to_event_location(ZONE).expect("event loc");
        let edat = EventDat::parse(&std::fs::read(eloc.path_under(root.root())).expect("read"))
            .expect("parse event dat");
        let sfid = ffxi_dat::zone_dat::zone_id_to_string_file_id(ZONE).expect("string file id");
        let sloc = root.resolve(sfid).expect("resolve string dat");
        let strings = StringDat::parse(
            &std::fs::read(sloc.path_under(root.root())).expect("read string dat"),
        )
        .expect("parse string dat");

        let block = edat.block_for_actor(HARARA).expect("harara block");
        let mut runner = DialogRunner::start(block, TALK_EVENT, ACT_INDEX).expect("event 32759");

        let mut end_para = None;
        for _ in 0..32 {
            match runner.advance(Some(0), &strings) {
                DialogStep::Frame(_) => {}
                DialogStep::Ended { end_para: ep } => {
                    end_para = Some(ep);
                    break;
                }
                DialogStep::Stopped(op) => panic!("event 32759 stopped on opcode 0x{op:02X}"),
            }
        }
        assert_eq!(end_para, Some(1), "Signet pick must return EndPara == 1");
    }
}
