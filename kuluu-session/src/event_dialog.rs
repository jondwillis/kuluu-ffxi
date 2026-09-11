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
use ffxi_dat::DatRoot;
use ffxi_event::{ActorLookup, DialogRunner, DialogStep, EventCue};
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
        if let Some(position) = self.player_position {
            runner.attach_scene(dat.clone(), block.actor, position);
        }
        let step = runner.advance(None, strings);
        self.scene_actions.extend(runner.take_scene_actions());
        self.cues.extend(
            runner
                .take_cues()
                .into_iter()
                .map(|c| resolve_cue(c, unique_no)),
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
                let dialog = frame_to_dialog(&active, frame, &self.player_name);
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
        self.drive(|runner, strings| runner.tick(dt_secs, strings))
    }

    fn drive(&mut self, step: impl FnOnce(&mut DialogRunner, &StringDat) -> DialogStep) -> Advance {
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
        let outcome = step(runner, strings);
        let final_position = runner.controlled_position();
        self.scene_actions.extend(runner.take_scene_actions());
        let cues: Vec<ResolvedCue> = runner
            .take_cues()
            .into_iter()
            .map(|c| resolve_cue(c, event_entity))
            .collect();
        let advance = match outcome {
            DialogStep::Frame(frame) => {
                Advance::Frame(frame_to_dialog(active, frame, &self.player_name))
            }
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
    }

    /// Entry `index` of `zone`'s dialog DAT, loading it if needed. `None` when
    /// the zone has no available string DAT (missing FFXI_DAT_PATH, unmapped
    /// zone) or the index is out of range.
    pub fn zone_text(&mut self, zone: u16, index: usize) -> Option<String> {
        self.ensure_strings(zone);
        self.strings.as_ref()?.text(index)
    }

    /// [`Self::zone_text`] restricted to entries that are actually printable
    /// lines. An entry carrying a Selection control code is a menu — prompt plus
    /// options — which retail drives through the event VM and never prints as
    /// chat, so a chat packet naming one means the server's text ids and this
    /// install's dialog DAT disagree about where the block starts.
    ///
    /// That happens whenever the two were built for different client eras: on
    /// the LandSandBoat pin under `vendor/`, the fishing block sits 8-10 entries
    /// above where a May-2023 install has it, so every fishing line would render
    /// as whatever entry now occupies that index. Returning `None` keeps the
    /// caller's placeholder — visibly wrong beats plausibly wrong.
    pub fn zone_chat_text(&mut self, zone: u16, index: usize) -> Option<String> {
        self.ensure_strings(zone);
        let dat = self.strings.as_ref()?;
        if dat.menu(index).is_some() {
            tracing::warn!(
                zone,
                index,
                "zone message names a menu entry, not a line — server text ids and the \
                 installed dialog DAT are from different client eras"
            );
            return None;
        }
        dat.text(index)
    }

    /// Resolve a TALKNUM-family message as a fishing line, reconciling the
    /// client-era skew between the server's text ids and this install's
    /// dialog DAT. The wire id is `server_base + offset`; the DAT entry lives
    /// at `install_base + offset`, and the two bases differ whenever the
    /// server and the install were built for different client eras (the
    /// vendor LSB pin sits ~9 entries above a May-2023 install). Without
    /// reconciliation every fishing line renders as whatever entry the skew
    /// lands on — another line entirely, or the menu-guard placeholder.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedCue {
    /// Crosses the wire boundary as a [`CutsceneCue`].
    Scene(CutsceneCue),
    /// 0x5D rides the existing [`AgentEvent::MusicVolumeChanged`] instead of
    /// the cue stream.
    MusicVolume { volume: u8, fade_frames: u16 },
}

/// Resolve one VM cue against `event_entity`, the server id of the entity the
/// running event belongs to.
pub fn resolve_cue(cue: EventCue, event_entity: u32) -> ResolvedCue {
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
        EventCue::ActorHide { target, hide } => CutsceneCue::ActorHide {
            target: actor(target),
            hide,
        },
        EventCue::CameraLock { lock } => CutsceneCue::CameraLock { lock },
        EventCue::Mount {
            target,
            status_event,
            mount_id,
        } => CutsceneCue::Mount {
            target: actor(target),
            status_event,
            mount_id,
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
/// DAT and the server's text ids and still count as the same block. Observed:
/// 9 between the vendor LSB pin and a May-2023 install, 26 between an older
/// server fork and that install.
const MAX_ERA_SKEW: u16 = 96;

/// Locate the fishing block in an installed dialog DAT by its landmark lines,
/// returning the block's base — the index LSB calls FISHING_MESSAGE_OFFSET.
/// One landmark could collide with another block's duplicate line, so the
/// match requires three lines at their exact relative offsets.
fn find_fishing_block(dat: &StringDat) -> Option<u16> {
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
        custom_menu: false,
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
    let Some(loc) = ffxi_dat::event_locate::zone_id_to_event_location(zone) else {
        tracing::warn!(
            zone,
            "no event DAT mapping for zone; NPC dialog disabled for this zone"
        );
        return None;
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
        let dialog = frame_to_dialog(&active, frame, "Zeid");
        assert_eq!(dialog.nums, vec![0, 250]);
        assert_eq!(dialog.prompt.as_deref(), Some("Trion: 250 gil, Zeid."));
        assert_eq!(dialog.choices, vec!["Pay 250.", "Decline."]);
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
}
