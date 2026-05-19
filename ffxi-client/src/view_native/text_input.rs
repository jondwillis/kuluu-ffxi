//! Mode-aware keyboard router for the native viewer.
//!
//! This system owns every key that has different meaning depending on
//! whether the user is walking the world, typing in chat, or navigating
//! a menu. Today's `view_native::input` continues to handle world-mode
//! movement (WASD / arrows / Tab / Esc-clear-target) on the
//! `ButtonInput<KeyCode>` resource — that surface is good for "is this
//! key currently held?" queries. We instead read the raw [`KeyboardInput`]
//! event stream, which carries the printable character / logical key per
//! press and is the right surface for text entry.
//!
//! # Mode transitions (compact scheme)
//!
//! ```text
//! World ──Space/`/`──> Chat   ──Enter──> (submit) ──> World
//!  │                    │
//!  │                    └─Esc(empty)─> World
//!  │
//!  ├──`-`──> Menu       ──Esc(root)──> World
//!  │
//!  └──Enter (target=Some) ─> Action::Talk dispatched
//!  └──Enter (target=None) ─> QuickAction
//! ```
//!
//! World-mode triggers are consumed silently (we don't append the
//! triggering `/` to a freshly-opened chat buffer twice — the
//! transition itself seeds the buffer). Chat-mode keys mutate the
//! buffer in-place. Slash-command parsing happens at submit time, not
//! per-keystroke, so the user can see and edit their command before
//! sending it.

use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::{PresentMode, PrimaryWindow};
use ffxi_viewer_core::dat_mmb::LoadMmbRequest;
use ffxi_viewer_core::dat_mzb::LoadMzbRequest;
use ffxi_viewer_core::hud::chat_panel::ChatScroll;
use ffxi_viewer_core::{
    Action, Bindings, ChatBuffer, DialogCursor, InputMode, MenuKind, MenuStack, Preset,
    QuickActionState, SceneState, Target, DIALOG_MAX_CHOICE,
};

use super::debug_heights::DebugHeightsRequest;

/// Tracks `/capture` toggle state and snapshots the framepace limiter
/// that was active *before* capture-mode was enabled, so `/capture off`
/// can restore it (otherwise an operator who had `/fps 30` active loses
/// that setting when toggling capture-mode).
///
/// Why this exists at all: on macOS, QuickTime's legacy screen-capture
/// pipeline can deadlock a Bevy/wgpu Metal surface while
/// `bevy_framepace` is parking the render thread against monitor
/// refresh. Capture-mode disables the limiter and pins
/// `PresentMode::Fifo` for the recording window.
#[derive(Resource, Default)]
pub struct CaptureMode {
    pub active: bool,
    /// `Some` while `active == true`; `None` otherwise. Stores the
    /// limiter that was in effect at the moment capture was enabled.
    pub restore_limiter: Option<bevy_framepace::Limiter>,
}

/// Bundle of `MessageWriter`s used by the slash-command dispatcher.
/// Grouped into one `SystemParam` so `text_input_system` stays under
/// Bevy's 16-param cap as we add more event-driven slash commands.
#[derive(SystemParam)]
pub struct SlashWriters<'w, 's> {
    pub load_mmb: MessageWriter<'w, LoadMmbRequest>,
    pub load_mzb: MessageWriter<'w, LoadMzbRequest>,
    pub debug_heights: MessageWriter<'w, DebugHeightsRequest>,
    /// `/logout` / `/shutdown` arming variants emit this so the HUD can
    /// start an optimistic countdown the instant the operator presses
    /// Enter, rather than waiting for the server's first 0x053 SYSTEMMES
    /// (which may never come if the validator silently rejects).
    pub logout_requested:
        MessageWriter<'w, ffxi_viewer_core::hud::logout_countdown::LogoutRequested>,
    // `/fps` mutates this directly. Bundled here (rather than a top-level
    // `ResMut` on `text_input_system`) to stay under Bevy's 16-SystemParam
    // cap. `Mut` access is fine — only the chat submit path writes it.
    pub framepace: ResMut<'w, bevy_framepace::FramepaceSettings>,
    /// `/capture` reconfigures the primary window's `present_mode` at
    /// runtime — Bevy's wgpu backend reconfigures the surface on the
    /// next frame when this field changes.
    pub primary_window: Query<'w, 's, &'static mut Window, With<PrimaryWindow>>,
    /// Persisted capture-toggle state — see [`CaptureMode`].
    pub capture_mode: ResMut<'w, CaptureMode>,
    /// `/bgm <id>` synthesizes a `ViewerEvent::MusicChanged` into
    /// the EventLog so the audio plugin's existing drain → resolve
    /// → decode pipeline plays the requested track. The EventLog
    /// resource is the same buffer `ingest_system` populates from
    /// real wire events; pushing here is indistinguishable
    /// downstream.
    pub event_log: ResMut<'w, ffxi_viewer_core::EventLog>,
    /// `/sfx <id>` writes directly into the audio plugin's SFX
    /// message queue. `SfxEvent::new(id)` plays once at full
    /// volume; `play_sfx_system` handles the decode + spawn.
    pub sfx_event: MessageWriter<'w, ffxi_viewer_core::audio::SfxEvent>,
}
use tokio::sync::mpsc::Sender;

use crate::keybinds_store::KeybindsStateRes;
use crate::state::{ActionKind, AgentCommand, AgentEvent, CheckKind, ReqLogoutKind};
use crate::view_native::input::CommandTx;
use crate::view_native::slash_commands::{
    parse_slash, system_chat_line, KeybindUpdate, SlashOutcome,
};

/// Read `KeyboardInput` events and route per [`InputMode`]. Runs every
/// `Update` tick. Cmd+Q / window-close are handled in `input.rs`'s
/// `handle_input_system` — keeping them there means quitting works even
/// while a UI is focused.
pub fn text_input_system(
    mut events: MessageReader<KeyboardInput>,
    cmd_tx: Res<CommandTx>,
    mut bindings: ResMut<Bindings>,
    mut keybinds_state: ResMut<KeybindsStateRes>,
    mut mode: ResMut<InputMode>,
    mut target: ResMut<Target>,
    mut scene_state: ResMut<SceneState>,
    mut exit: MessageWriter<AppExit>,
    mut navmesh_visible: ResMut<super::navmesh_overlay::NavmeshOverlayVisible>,
    navmesh_state: Res<super::navmesh_overlay::NavmeshState>,
    // `/agent` control surface. Resources are optional: `AgentPaused`
    // only exists when `--agent-listen` is configured, and
    // `SessionEventTx` only after the connecting bridge runs.
    #[cfg(unix)] agent_paused: Option<Res<super::AgentPaused>>,
    session_event_tx: Option<Res<super::SessionEventTx>>,
    // Bundle of slash-command event writers (load_mmb / load_mzb /
    // debug_heights). Bundled into one SystemParam so this system stays
    // under Bevy's 16-param cap as the slash-command surface grows.
    mut slash_writers: SlashWriters,
    // Operator-tunable cull distances + zonegeom visibility flag.
    // `/drawdistance` mutates the radii; `/zonegeom` flips the bool.
    // Bundled into one resource to stay under Bevy's 16-SystemParam
    // limit at this dispatcher site.
    mut draw_distance: ResMut<ffxi_viewer_core::dat_mzb::DrawDistance>,
    // Chat-panel scroll offset, mutated by PassiveCursor arrow/PageUp
    // keys here and by the wheel system in `hud::chat_panel`. Single
    // source of truth so both inputs stay in sync.
    mut chat_scroll: ResMut<ChatScroll>,
) {
    // Snapshot the inputs we need from the scene before mutating it
    // below (clones are cheap relative to the per-keystroke event
    // surface, and they free the borrow so `apply_chat_action` can
    // append system chat lines into the same resource).
    let entities = scene_state.snapshot.entities.clone();
    let self_pos = scene_state.snapshot.self_pos.pos;
    let current_target = target.id;

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &mut *mode {
            InputMode::World => {
                // Dead-state intercept: when our own party row reports
                // 0% HP, the dead-state HUD prompt is up; route Enter
                // straight to `ReturnToHomePoint` instead of opening
                // the contextual action menu (which would be useless
                // — you can't act while K.O.'d). Mirrors the retail
                // "press Enter to release" muscle memory.
                if ffxi_viewer_core::hud::death_prompt::is_dead(&scene_state)
                    && bindings.matches_logical(Action::ConfirmAction, &ev.logical_key)
                {
                    if let Err(e) = cmd_tx.0.try_send(AgentCommand::ReturnToHomePoint) {
                        push_system_chat_line(
                            &mut scene_state,
                            format!("/return dropped (channel issue): {e}"),
                        );
                    }
                    continue;
                }
                if let Some(next) = handle_world_key(
                    &ev.logical_key,
                    &bindings,
                    current_target,
                    &entities,
                    self_pos,
                    &mut target,
                ) {
                    *mode = next;
                }
            }
            InputMode::Chat(buffer) => {
                let action = handle_chat_key(&ev.logical_key, &bindings, buffer);
                apply_chat_action(
                    action,
                    &mut mode,
                    &entities,
                    self_pos,
                    current_target,
                    &mut target,
                    &cmd_tx.0,
                    &mut scene_state,
                    &mut exit,
                    &mut navmesh_visible,
                    &navmesh_state,
                    &mut bindings,
                    &mut keybinds_state,
                    #[cfg(unix)]
                    agent_paused.as_deref(),
                    session_event_tx.as_deref(),
                    &mut slash_writers,
                    &mut draw_distance,
                );
            }
            InputMode::Menu(stack) => {
                if let Some(next) = handle_menu_key(
                    &ev.logical_key,
                    &mut bindings,
                    stack,
                    &mut scene_state,
                    &cmd_tx.0,
                    &mut keybinds_state,
                ) {
                    *mode = next;
                }
            }
            InputMode::QuickAction(qa) => {
                if let Some(next) = handle_quick_action_key(
                    &ev.logical_key,
                    &bindings,
                    qa,
                    &mut scene_state,
                    current_target,
                    &entities,
                    &cmd_tx.0,
                ) {
                    *mode = next;
                }
            }
            InputMode::Dialog(cursor) => {
                if let Some(next) = handle_dialog_key(
                    &ev.logical_key,
                    &bindings,
                    cursor,
                    &mut scene_state,
                    &cmd_tx.0,
                ) {
                    *mode = next;
                }
            }
            InputMode::PassiveCursor(_state) => {
                if let Some(next) = handle_passive_cursor_key(
                    &ev.logical_key,
                    &bindings,
                    &mut chat_scroll,
                    &scene_state,
                ) {
                    *mode = next;
                }
            }
        }
    }
}

/// PreUpdate sync: drive `InputMode` between `World` and `Dialog` based
/// on whether `SceneState.snapshot.dialog` is `Some`. Runs every frame so
/// the dialog HUD's input mode tracks the server's event lifecycle
/// without leaking dialog responsibility into every system.
///
/// Conservative on transitions: only swaps `World ↔ Dialog`. If the user
/// is in `Chat`/`Menu`/`QuickAction` when an event fires, those modes
/// stay until they exit naturally — better than yanking the operator
/// out of typed text mid-keystroke.
pub fn dialog_mode_sync_system(state: Res<SceneState>, mut mode: ResMut<InputMode>) {
    let dialog_open = state.snapshot.dialog.is_some();
    match (&*mode, dialog_open) {
        (InputMode::World, true) => {
            *mode = InputMode::Dialog(DialogCursor::default());
        }
        (InputMode::Dialog(_), false) => {
            *mode = InputMode::World;
        }
        _ => {}
    }
}

/// World-mode triggers. Returns `Some(new_mode)` to transition; `None`
/// to stay in world. Pure router — Enter no longer dispatches Talk
/// directly (it opens the contextual menu instead), so this function
/// no longer needs the command sender.
fn handle_world_key(
    key: &Key,
    bindings: &Bindings,
    current_target: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    self_pos: ffxi_viewer_wire::Vec3,
    target: &mut Target,
) -> Option<InputMode> {
    // OpenChat (default Space) stays in this logical-key router rather
    // than moving to input.rs's physical-key path: Space arrives as
    // `Key::Space`, and if input.rs handled it, the same KeyboardInput
    // event would still be routed to handle_chat_key on the same frame
    // and push a leading space into the buffer. Matching here lets us
    // *swallow* the Space event by transitioning out of World before the
    // event is re-dispatched.
    if bindings.matches_logical(Action::OpenChat, key) {
        // Space (default OpenChat) is *swallowed* — we transition out of
        // World here so the same KeyboardInput event isn't re-routed to
        // handle_chat_key on this frame (which would push a leading
        // space into the buffer). NOTE: `/` (OpenChatCommand) and `-`
        // (OpenMenu) live in `input.rs`'s physical-key reader for the
        // same reason but with the *opposite* desired effect — for `/`
        // we want the slash to land in the buffer, so input.rs sets
        // mode then text_input.rs's chat handler appends it naturally.
        return Some(InputMode::Chat(ChatBuffer::empty()));
    }
    if bindings.matches_logical(Action::ConfirmAction, key) {
        // Enter (default ConfirmAction):
        // - With a current target → open the contextual menu (FFXI-retail:
        //   Enter on a target brings up the action ring, not a direct
        //   Talk). The operator picks Attack/Check/Talk/etc from there.
        // - With no target + nearby NPC → auto-acquire the NPC (no menu
        //   yet). The next Enter press will then open the menu against
        //   that target. This matches the retail "step up to NPC and
        //   press Enter" muscle-memory.
        // - With no target + no nearby NPC → open the menu directly.
        return match current_target {
            Some(_) => Some(InputMode::QuickAction(QuickActionState::for_target(true))),
            None => match nearest_npc(entities, self_pos, AUTO_TARGET_RADIUS) {
                Some(ent) => {
                    target.id = Some(ent.id);
                    None
                }
                None => Some(InputMode::QuickAction(QuickActionState::for_target(false))),
            },
        };
    }
    None
}

/// Yalms within which Enter-with-no-target auto-acquires the nearest
/// NPC. 8 yalms is roughly the retail "interact with NPC" range; the
/// auto-target should match that so the operator's mental model of
/// "Enter near an NPC" works the same way it does in-game.
const AUTO_TARGET_RADIUS: f32 = 8.0;

/// Find the nearest NPC entity within `radius` yalms. Returns `None`
/// if no NPC qualifies — the caller falls back to the quick-action
/// picker in that case. Mobs/PCs are skipped since `Enter` for those
/// shouldn't be silently auto-stolen by an in-range mob.
fn nearest_npc<'a>(
    entities: &'a [ffxi_viewer_wire::Entity],
    self_pos: ffxi_viewer_wire::Vec3,
    radius: f32,
) -> Option<&'a ffxi_viewer_wire::Entity> {
    let r2 = radius * radius;
    entities
        .iter()
        .filter(|e| matches!(e.kind, ffxi_viewer_wire::EntityKind::Npc))
        .map(|e| {
            let dx = e.pos.x - self_pos.x;
            let dy = e.pos.y - self_pos.y;
            (e, dx * dx + dy * dy)
        })
        .filter(|(_, d2)| *d2 <= r2)
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(e, _)| e)
}

/// Result of a chat-mode keystroke. `Stay` keeps the buffer; `Submit`
/// triggers slash parsing or a Say chat send; `Exit` returns to World.
enum ChatAction {
    Stay,
    Submit,
    Exit,
}

fn handle_chat_key(key: &Key, bindings: &Bindings, buffer: &mut ChatBuffer) -> ChatAction {
    // Submit/Exit/Backspace go through bindings so a future user can
    // remap them; the printable-character branch and the literal Space
    // branch stay raw because they're text input, not action triggers.
    if bindings.matches_logical(Action::ChatSubmit, key) {
        return ChatAction::Submit;
    }
    if bindings.matches_logical(Action::ChatExit, key) {
        return if buffer.text.is_empty() {
            ChatAction::Exit
        } else {
            buffer.text.clear();
            ChatAction::Stay
        };
    }
    if bindings.matches_logical(Action::ChatBackspace, key) {
        buffer.text.pop();
        return ChatAction::Stay;
    }
    match key {
        Key::Space => {
            buffer.text.push(' ');
            ChatAction::Stay
        }
        Key::Character(s) => {
            for c in s.chars() {
                if !c.is_control() {
                    buffer.text.push(c);
                }
            }
            ChatAction::Stay
        }
        _ => ChatAction::Stay,
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn apply_chat_action(
    action: ChatAction,
    mode: &mut InputMode,
    entities: &[ffxi_viewer_wire::Entity],
    self_pos: ffxi_viewer_wire::Vec3,
    current_target: Option<u32>,
    target: &mut Target,
    cmd_tx: &Sender<AgentCommand>,
    scene_state: &mut SceneState,
    exit: &mut MessageWriter<AppExit>,
    navmesh_visible: &mut super::navmesh_overlay::NavmeshOverlayVisible,
    navmesh_state: &super::navmesh_overlay::NavmeshState,
    bindings: &mut Bindings,
    keybinds_state: &mut KeybindsStateRes,
    #[cfg(unix)] agent_paused: Option<&super::AgentPaused>,
    session_event_tx: Option<&super::SessionEventTx>,
    slash_writers: &mut SlashWriters,
    draw_distance: &mut ffxi_viewer_core::dat_mzb::DrawDistance,
) {
    match action {
        ChatAction::Stay => {}
        ChatAction::Exit => {
            *mode = InputMode::World;
        }
        ChatAction::Submit => {
            // Pull the buffer out before we mutate `mode`.
            let buffer = match mode {
                InputMode::Chat(b) => std::mem::take(&mut b.text),
                _ => return,
            };
            let trimmed = buffer.trim();
            if trimmed.is_empty() {
                *mode = InputMode::World;
                return;
            }
            if trimmed.starts_with('/') {
                let outcome = parse_slash(
                    trimmed,
                    entities,
                    self_pos,
                    current_target,
                    scene_state.snapshot.zone_id,
                );
                tracing::debug!(buffer = %trimmed, outcome = ?outcome, "chat submit: slash");
                // Locally echo /say & friends so the operator sees their
                // own line immediately, independent of server echo timing.
                match &outcome {
                    SlashOutcome::Command(AgentCommand::Chat { kind, text }) => {
                        push_local_chat_line(scene_state, *kind, text.clone());
                    }
                    // Outbound /tell: echo with `sender = recipient` so
                    // the panel renders ">>Daisy : msg" — matches the
                    // retail-client outbound display shape.
                    SlashOutcome::Command(AgentCommand::Tell { to, text }) => {
                        push_local_tell_echo(scene_state, to.clone(), text.clone());
                    }
                    _ => {}
                }
                apply_slash_outcome(
                    outcome,
                    target,
                    cmd_tx,
                    scene_state,
                    exit,
                    navmesh_visible,
                    navmesh_state,
                    self_pos,
                    bindings,
                    keybinds_state,
                    #[cfg(unix)]
                    agent_paused,
                    session_event_tx,
                    slash_writers,
                    draw_distance,
                );
            } else {
                // Default channel: Say. Tracing makes it visible whether the
                // dispatch reached the session loop — the server's 0x017 echo
                // is what eventually populates the chat panel; if the user
                // never sees their own line back, this log narrows where
                // the loop broke.
                tracing::debug!(text = %trimmed, "chat submit: say");
                // Local echo with `kind=0` (Say) — the same path the
                // server echo would land in, so the operator sees their
                // line in the chat panel right when they hit Enter.
                push_local_chat_line(scene_state, 0, trimmed.to_string());
                let send_result = cmd_tx.try_send(AgentCommand::Chat {
                    kind: 0,
                    text: trimmed.to_string(),
                });
                if let Err(e) = send_result {
                    push_system_chat_line(
                        scene_state,
                        format!("chat dropped (channel issue): {e}"),
                    );
                }
            }
            *mode = InputMode::World;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_slash_outcome(
    outcome: SlashOutcome,
    target: &mut Target,
    cmd_tx: &Sender<AgentCommand>,
    scene_state: &mut SceneState,
    exit: &mut MessageWriter<AppExit>,
    navmesh_visible: &mut super::navmesh_overlay::NavmeshOverlayVisible,
    navmesh_state: &super::navmesh_overlay::NavmeshState,
    self_pos: ffxi_viewer_wire::Vec3,
    bindings: &mut Bindings,
    keybinds_state: &mut KeybindsStateRes,
    #[cfg(unix)] agent_paused: Option<&super::AgentPaused>,
    session_event_tx: Option<&super::SessionEventTx>,
    slash_writers: &mut SlashWriters,
    draw_distance: &mut ffxi_viewer_core::dat_mzb::DrawDistance,
) {
    match outcome {
        SlashOutcome::Command(cmd) => {
            if let Some(toast) = reqlogout_ack_text(&cmd) {
                push_system_chat_line(scene_state, toast.into());
            }
            if let Some(shutdown) = reqlogout_starts_countdown(&cmd) {
                slash_writers.logout_requested.write(
                    ffxi_viewer_core::hud::logout_countdown::LogoutRequested { shutdown },
                );
            }
            let send_result = cmd_tx.try_send(cmd);
            if let Err(e) = send_result {
                // Channel full or closed — operator should see this; silent
                // drops are how slash bugs hide. The text goes straight to
                // the chat panel via the same path /say already uses.
                push_system_chat_line(scene_state, format!("command dropped (channel issue): {e}"));
            }
        }
        SlashOutcome::Commands(cmds) => {
            // Order-sensitive: e.g. `/logout` → [ReqLogout, Heal On], and
            // we want the wire flush of ReqLogout to land first so the
            // 30s server timer arms even if the channel back-pressures
            // and Heal silently drops. Each drop is reported
            // individually with the same toast Command uses.
            for cmd in cmds {
                if let Some(toast) = reqlogout_ack_text(&cmd) {
                    push_system_chat_line(scene_state, toast.into());
                }
                if let Some(shutdown) = reqlogout_starts_countdown(&cmd) {
                    slash_writers.logout_requested.write(
                        ffxi_viewer_core::hud::logout_countdown::LogoutRequested { shutdown },
                    );
                }
                if let Err(e) = cmd_tx.try_send(cmd) {
                    push_system_chat_line(
                        scene_state,
                        format!("command dropped (channel issue): {e}"),
                    );
                }
            }
        }
        SlashOutcome::SetTarget(id) => {
            target.id = id;
        }
        SlashOutcome::Quit => {
            let _ = cmd_tx.try_send(AgentCommand::Disconnect);
            exit.write_default();
        }
        SlashOutcome::QuitWithLogout(kind) => {
            // Order matters: enqueue ReqLogout *before* Disconnect.
            // Both go through the same single-consumer `cmd_rx`, so the
            // session loop processes ReqLogout first (awaiting
            // `send_encrypted` flushes the 0x0E7 to the wire), then
            // Disconnect breaks the loop. AppExit closes the window
            // either way — even if the channel is full and one of these
            // gets dropped, the user still gets the local exit they
            // asked for.
            let req = AgentCommand::ReqLogout { kind };
            if let Some(toast) = reqlogout_ack_text(&req) {
                push_system_chat_line(scene_state, toast.into());
            }
            if let Some(shutdown) = reqlogout_starts_countdown(&req) {
                slash_writers.logout_requested.write(
                    ffxi_viewer_core::hud::logout_countdown::LogoutRequested { shutdown },
                );
            }
            let _ = cmd_tx.try_send(req);
            let _ = cmd_tx.try_send(AgentCommand::Disconnect);
            exit.write_default();
        }
        SlashOutcome::SystemMessage(text) => {
            // Split on '\n' so multi-line outputs (`/help`, `/zones`)
            // become separate ChatLines instead of one giant wrapping
            // row. Single-line messages still go through unchanged
            // because `split('\n')` yields exactly one item then.
            for line in text.split('\n') {
                push_system_chat_line(scene_state, line.to_string());
            }
        }
        SlashOutcome::ToggleNavmesh(setting) => {
            let next = setting.unwrap_or(!navmesh_visible.0);
            navmesh_visible.0 = next;
            let label = if next { "ON" } else { "OFF" };
            push_system_chat_line(scene_state, format!("navmesh overlay: {label}"));
        }
        SlashOutcome::LoadMmb {
            file_id,
            chunk_idx,
            world_pos,
            entity_id,
        } => {
            // Cross the FFXI→Bevy axis flip here, in the dispatcher, so
            // `LoadMmbRequest::world_pos` is already a Bevy `Vec3` by
            // the time the consumer system sees it. Matches the
            // convention used by `sync_entities_system` (`scene.rs`).
            // When `entity_id` is `Some`, `world_pos` is ignored
            // downstream — the consumer hangs the mesh under the
            // tracked entity instead.
            let bevy_pos = ffxi_viewer_core::ffxi_to_bevy(world_pos);
            slash_writers.load_mmb.write(LoadMmbRequest {
                file_id,
                chunk_idx,
                world_pos: bevy_pos,
                entity_id,
                world_transform: None,
            });
            let label = match entity_id {
                Some(id) => format!("/load_mmb_on {id} {file_id} {chunk_idx}: spawning…"),
                None => format!("/load_mmb {file_id} {chunk_idx}: spawning…"),
            };
            push_system_chat_line(scene_state, label);
        }
        SlashOutcome::DebugHeights => {
            slash_writers.debug_heights.write(DebugHeightsRequest);
        }
        SlashOutcome::PlayBgm { track_id } => {
            // Synthesize the same wire event a 0x05F packet would
            // produce — slot 0 (ZoneDay) is the default audible slot
            // when nothing else is set. The audio plugin's
            // drain_music_events_system folds this into BgmSlots
            // and apply_bgm_system decodes + plays.
            slash_writers
                .event_log
                .recent
                .push_back(ffxi_viewer_wire::ViewerEvent::MusicChanged {
                    slot: 0,
                    track_id,
                });
            push_system_chat_line(scene_state, format!("/bgm {track_id}: queued"));
        }
        SlashOutcome::PlaySfx { se_id } => {
            slash_writers
                .sfx_event
                .write(ffxi_viewer_core::audio::SfxEvent::new(se_id));
            push_system_chat_line(scene_state, format!("/sfx {se_id}: fired"));
        }
        SlashOutcome::EndCutscene { event_num } => {
            // Resolve player's own UniqueNo + ActIndex. For a forced
            // cutscene fired by `player:startEvent(csid, ...)` the server
            // built the EVENTSTART with the player as the initiator, so
            // 0x05B EVENT_END targeting `(self_char_id, self_act_index,
            // csid)` reaches LSB's `onEventFinish[csid]` handler — which
            // is what clears `New_Character_Cutscenes.lua`'s `notSeen`
            // var and (more importantly) `PChar->m_event`.
            let self_char_id = scene_state.snapshot.self_char_id;
            let self_act_index = self_char_id.and_then(|id| {
                scene_state
                    .snapshot
                    .entities
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.act_index)
            });
            match (self_char_id, self_act_index) {
                (Some(event_id), Some(act_index)) => {
                    push_system_chat_line(
                        scene_state,
                        format!(
                            "/endcutscene: sending EVENT_END (csid={event_num}, \
                             unique_no=0x{event_id:08X}, act_index={act_index})"
                        ),
                    );
                    if let Err(e) = cmd_tx.try_send(AgentCommand::EndEventChoice {
                        event_id,
                        act_index,
                        event_num,
                        choice: 0,
                    }) {
                        push_system_chat_line(
                            scene_state,
                            format!("/endcutscene: dropped (channel issue): {e}"),
                        );
                    }
                }
                _ => {
                    push_system_chat_line(
                        scene_state,
                        "/endcutscene: self entity not in snapshot yet — wait for \
                         zone-in to complete and retry"
                            .into(),
                    );
                }
            }
        }
        SlashOutcome::SetTargetFps(target) => {
            use bevy_framepace::Limiter;
            match target {
                Some(n) => {
                    slash_writers.framepace.limiter = Limiter::from_framerate(n as f64);
                    push_system_chat_line(scene_state, format!("/fps: capped at {n}"));
                }
                None => {
                    slash_writers.framepace.limiter = Limiter::Off;
                    push_system_chat_line(scene_state, "/fps: cap disabled".into());
                }
            }
        }
        SlashOutcome::SetCaptureMode(arg) => {
            use bevy_framepace::Limiter;
            let want_on = arg.unwrap_or(!slash_writers.capture_mode.active);
            if want_on == slash_writers.capture_mode.active {
                let label = if want_on { "on" } else { "off" };
                push_system_chat_line(
                    scene_state,
                    format!("/capture: already {label} (no change)"),
                );
            } else if want_on {
                // Snapshot the framepace limiter so `/capture off` can
                // restore the operator's `/fps` choice. Then disable
                // framepace and pin Fifo on the primary window.
                slash_writers.capture_mode.restore_limiter =
                    Some(slash_writers.framepace.limiter.clone());
                slash_writers.framepace.limiter = Limiter::Off;
                if let Ok(mut window) = slash_writers.primary_window.single_mut() {
                    window.present_mode = PresentMode::Fifo;
                }
                slash_writers.capture_mode.active = true;
                push_system_chat_line(
                    scene_state,
                    "/capture: on (framepace off, present_mode=Fifo) — \
                     prefer OBS/Cmd+Shift+5 over QuickTime if recording still stalls"
                        .into(),
                );
            } else {
                let restored = slash_writers
                    .capture_mode
                    .restore_limiter
                    .take()
                    .unwrap_or(Limiter::Auto);
                slash_writers.framepace.limiter = restored;
                if let Ok(mut window) = slash_writers.primary_window.single_mut() {
                    window.present_mode = PresentMode::AutoVsync;
                }
                slash_writers.capture_mode.active = false;
                push_system_chat_line(scene_state, "/capture: off (settings restored)".into());
            }
        }
        SlashOutcome::SetZoneGeom(setting) => {
            let next = setting.unwrap_or_else(|| draw_distance.zone_geom_mode.cycle());
            draw_distance.zone_geom_mode = next;
            push_system_chat_line(scene_state, format!("/zonegeom: {}", next.label()));
        }
        SlashOutcome::SetDrawDistance(op) => {
            use super::slash_commands::DrawDistanceOp;
            match op {
                DrawDistanceOp::Show => {
                    push_system_chat_line(
                        scene_state,
                        format!(
                            "/drawdistance world={:.0} mob={:.0} (yalms)",
                            draw_distance.world, draw_distance.mob
                        ),
                    );
                }
                DrawDistanceOp::SetWorld(v) => {
                    draw_distance.world = v;
                    push_system_chat_line(
                        scene_state,
                        format!("/drawdistance: setworld {v:.0} yalms"),
                    );
                }
                DrawDistanceOp::SetMob(v) => {
                    draw_distance.mob = v;
                    push_system_chat_line(
                        scene_state,
                        format!("/drawdistance: setmob {v:.0} yalms"),
                    );
                }
            }
        }
        SlashOutcome::LoadMzb {
            file_id,
            chunk_idx,
            world_pos,
        } => {
            let bevy_pos = ffxi_viewer_core::ffxi_to_bevy(world_pos);
            slash_writers.load_mzb.write(LoadMzbRequest {
                file_id,
                chunk_idx,
                world_pos: bevy_pos,
                // Manual /load_mzb is *not* auto-load — survives zone
                // changes so the operator can compare zone-A debris with
                // zone-B's geometry side-by-side.
                auto_loaded: false,
            });
            let idx_desc = match chunk_idx {
                Some(i) => format!("chunk {i}"),
                None => "first MZB chunk".to_string(),
            };
            push_system_chat_line(
                scene_state,
                format!("/load_mzb {file_id} ({idx_desc}): spawning…"),
            );
        }
        SlashOutcome::ShopBuyRow { shop_index, qty } => {
            // Resolve the `shop_no` from the live shop's offset_index.
            // Without an open shop, the buy is a no-op with a system
            // toast — the operator sees why.
            match scene_state.snapshot.shop.as_ref() {
                Some(shop) => {
                    let _ = cmd_tx.try_send(AgentCommand::ShopBuy {
                        shop_no: shop.offset_index,
                        shop_index,
                        qty,
                    });
                }
                None => push_system_chat_line(scene_state, "/buy: no shop is open".into()),
            }
        }
        SlashOutcome::ApplyKeybinds(update) => {
            apply_keybind_update(update, bindings, keybinds_state, scene_state);
        }
        SlashOutcome::NavInfo => {
            report_nav_info(navmesh_state, self_pos, scene_state);
        }
        SlashOutcome::AgentControl(op) => {
            #[cfg(unix)]
            apply_agent_control(op, agent_paused, session_event_tx, scene_state);
            #[cfg(not(unix))]
            {
                let _ = op;
                push_system_chat_line(
                    scene_state,
                    "/agent: requires Unix-domain-socket build (non-Unix target)".into(),
                );
            }
        }
        SlashOutcome::CopyToasts { n } => {
            apply_copy_toasts(n, scene_state);
        }
    }
}

/// Copy the last `n` UI-local toasts (slash-command responses) to the OS
/// clipboard, newline-joined. `n` is clamped to the number of toasts that
/// actually exist — asking for more isn't an error.
///
/// The clipboard handle is constructed per call rather than cached: on
/// macOS `arboard` opens an `NSPasteboard` reference which is cheap,
/// and not keeping it alive avoids cross-thread `Send` headaches with
/// Bevy's system params. A failed open or write surfaces as a `[system]`
/// toast — the operator should know the copy didn't land rather than
/// silently believing the clipboard has stale content.
fn apply_copy_toasts(n: usize, scene_state: &mut SceneState) {
    let toasts = &scene_state.local_toasts;
    if toasts.is_empty() {
        push_system_chat_line(scene_state, "/copy: no toasts to copy".into());
        return;
    }
    let take = n.min(toasts.len());
    let start = toasts.len() - take;
    // Join with '\n'; the rendered chat panel already shows each toast
    // on its own row, so reproducing that line-break shape is what the
    // operator expects to land on their clipboard.
    let payload: String = toasts[start..]
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    match arboard::Clipboard::new() {
        Ok(mut cb) => match cb.set_text(payload) {
            Ok(()) => {
                push_system_chat_line(
                    scene_state,
                    format!("/copy: {take} toast(s) on clipboard"),
                );
            }
            Err(e) => {
                push_system_chat_line(scene_state, format!("/copy: clipboard write failed: {e}"));
            }
        },
        Err(e) => {
            push_system_chat_line(scene_state, format!("/copy: clipboard unavailable: {e}"));
        }
    }
}

/// Dispatcher for `/agent pause|resume|status`. Reads/writes the
/// `AgentPaused` atomic and fires `AgentEvent::HumanInControl` /
/// `HumanReleased` on transitions. Idempotent — re-pausing while
/// paused (or re-resuming while running) is a no-op and just prints
/// the current state.
#[cfg(unix)]
fn apply_agent_control(
    op: super::slash_commands::AgentControlOp,
    agent_paused: Option<&super::AgentPaused>,
    session_event_tx: Option<&super::SessionEventTx>,
    scene_state: &mut SceneState,
) {
    use super::slash_commands::AgentControlOp;
    use std::sync::atomic::Ordering;
    let Some(paused) = agent_paused else {
        push_system_chat_line(
            scene_state,
            "/agent: no agent attached (set --agent-listen to enable)".into(),
        );
        return;
    };
    match op {
        AgentControlOp::Pause => {
            let was_paused = paused.0.swap(true, Ordering::AcqRel);
            if was_paused {
                push_system_chat_line(scene_state, "/agent: already paused".into());
            } else {
                push_system_chat_line(scene_state, "/agent: paused (human in control)".into());
                if let Some(tx) = session_event_tx {
                    let _ = tx.0.send(AgentEvent::HumanInControl {
                        reason: "operator /agent pause".into(),
                    });
                }
            }
        }
        AgentControlOp::Resume => {
            let was_paused = paused.0.swap(false, Ordering::AcqRel);
            if !was_paused {
                push_system_chat_line(scene_state, "/agent: not currently paused".into());
            } else {
                push_system_chat_line(scene_state, "/agent: resumed".into());
                if let Some(tx) = session_event_tx {
                    let _ = tx.0.send(AgentEvent::HumanReleased);
                }
            }
        }
        AgentControlOp::Status => {
            let state = if paused.0.load(Ordering::Acquire) {
                "PAUSED (human in control)"
            } else {
                "RUNNING (agent in control)"
            };
            push_system_chat_line(scene_state, format!("/agent: {state}"));
        }
    }
}

/// `/navinfo` dispatcher: diagnostic snapshot of where the player is
/// relative to the navmesh and the zone-line list. Pushes one or more
/// `[system]` chat lines so the operator can see, without log
/// scraping, whether (a) the navmesh thinks they're on it, (b) their
/// height matches a real polygon, and (c) the zone lines they expect
/// to walk into are actually findable from here.
fn report_nav_info(
    navmesh_state: &super::navmesh_overlay::NavmeshState,
    self_pos: ffxi_viewer_wire::Vec3,
    scene_state: &mut SceneState,
) {
    let zone_id = scene_state.snapshot.zone_id;
    push_system_chat_line(
        scene_state,
        format!(
            "navinfo: self=(x={:.2} y={:.2} z={:.2}) zone={}",
            self_pos.x,
            self_pos.y,
            self_pos.z,
            zone_id.map_or("?".into(), |z| z.to_string()),
        ),
    );
    let Some(nav_arc) = navmesh_state.nav.as_ref() else {
        push_system_chat_line(
            scene_state,
            "navinfo: no navmesh loaded for current zone".into(),
        );
        return;
    };
    // `std::sync::Mutex::lock` returns `Result<_, PoisonError>`; treat
    // a poisoned mutex as a hard failure for this read-only diagnostic
    // (no need to recover the poisoned data — we'd just report stale).
    let nav = match nav_arc.lock() {
        Ok(g) => g,
        Err(_) => {
            push_system_chat_line(
                scene_state,
                "navinfo: navmesh mutex poisoned — bailing".into(),
            );
            return;
        }
    };
    let snap = nav.nearest_height_at(self_pos.x, self_pos.y, self_pos.z);
    match snap {
        Some(snapped_z) => {
            let delta_z = snapped_z - self_pos.z;
            push_system_chat_line(
                scene_state,
                format!(
                    "navinfo: nearest-poly z={:.2} (delta z={:+.2} yalms)",
                    snapped_z, delta_z
                ),
            );
        }
        None => push_system_chat_line(
            scene_state,
            "navinfo: NO walkable polygon within 100-yalm vertical search — self_pos appears off-mesh".into(),
        ),
    }
    // Distance to each zone line in this zone, plus whether path()
    // succeeds from here to the line. Caps at 6 lines to avoid spamming.
    if let Some(z) = zone_id {
        let lines = ffxi_nav::zone_lines_for(z);
        if lines.is_empty() {
            push_system_chat_line(scene_state, format!("navinfo: zone {z} has no zone-lines"));
            return;
        }
        let from = ffxi_nav::glam::Vec3::new(self_pos.x, self_pos.y, self_pos.z);
        for line in lines.iter().take(6) {
            let dx = line.from_pos[0] - self_pos.x;
            let dy = line.from_pos[1] - self_pos.y;
            let dz = line.from_pos[2] - self_pos.z;
            let dist_2d = (dx * dx + dy * dy).sqrt();
            let to =
                ffxi_nav::glam::Vec3::new(line.from_pos[0], line.from_pos[1], line.from_pos[2]);
            let path_status = match ffxi_nav::NavMesh::path(&*nav, from, to) {
                Some(p) => format!("path={}wp", p.len()),
                None => "path=NONE".into(),
            };
            let name = ffxi_nav::zone_name(line.to_zone).unwrap_or("?");
            push_system_chat_line(
                scene_state,
                format!(
                    "navinfo: →zone{:3} {:<20} dist={:.1}y dz={:+.1} {}",
                    line.to_zone, name, dist_2d, dz, path_status
                ),
            );
        }
    }
}

/// Apply a `/keybinds` subcommand: swap the [`Bindings`] resource,
/// persist the new state, and surface a system-chat confirmation. List
/// is a read-only print.
fn apply_keybind_update(
    update: KeybindUpdate,
    bindings: &mut Bindings,
    keybinds_state: &mut KeybindsStateRes,
    scene_state: &mut SceneState,
) {
    match update {
        KeybindUpdate::Preset(preset) => {
            let (new_bindings, save_result) = keybinds_state.apply_preset(preset);
            *bindings = new_bindings;
            push_system_chat_line(
                scene_state,
                format!("/keybinds: preset → {}", preset.slug()),
            );
            if let Err(e) = save_result {
                push_system_chat_line(scene_state, format!("/keybinds: save failed: {e}"));
            }
        }
        KeybindUpdate::Reset => {
            let preset = keybinds_state.persisted.preset;
            let (new_bindings, save_result) = keybinds_state.apply_reset();
            *bindings = new_bindings;
            push_system_chat_line(
                scene_state,
                format!("/keybinds: reset to {} defaults", preset.slug()),
            );
            if let Err(e) = save_result {
                push_system_chat_line(scene_state, format!("/keybinds: save failed: {e}"));
            }
        }
        KeybindUpdate::List => {
            push_system_chat_line(
                scene_state,
                format!(
                    "/keybinds: preset = {}",
                    keybinds_state.persisted.preset.slug()
                ),
            );
            // One line per (Action, KeyBind). BTreeMap iteration is
            // already sorted, so the output order is stable.
            for (action, bind) in bindings.iter() {
                let mods = format_modifiers(bind.mods);
                push_system_chat_line(scene_state, format!("  {action:?} → {mods}{:?}", bind.key));
            }
        }
    }
}

fn format_modifiers(mods: ffxi_viewer_core::Modifiers) -> &'static str {
    match (mods.ctrl, mods.alt, mods.shift, mods.super_) {
        (false, false, false, false) => "",
        (true, false, false, false) => "Ctrl+",
        (false, true, false, false) => "Alt+",
        (false, false, true, false) => "Shift+",
        (false, false, false, true) => "Super+",
        // Multi-modifier combos collapse to a generic prefix; rare
        // enough that listing them all out doesn't earn its keep.
        _ => "Mod+",
    }
}

/// Append a `[system]` chat line into the UI-local toast buffer so the
/// chat panel renders it. Toasts persist across snapshot replacement
/// (`SceneState::push_local_toast` → `local_toasts`), so a slash error
/// or "command dropped" stays visible until evicted by the cap rather
/// than vanishing on the very next ingest tick.
fn push_system_chat_line(scene_state: &mut SceneState, text: String) {
    scene_state.push_local_toast(system_chat_line(text));
}

/// If `cmd` is an `AgentCommand::ReqLogout`, return the local-toast text
/// the user should see immediately when the slash is dispatched.
///
/// Why: `/logout` and `/shutdown` are slow round-trips — the server's
/// `EXECUTING_LOGOUT` (msg id=7) / `EXECUTING_SHUTDOWN` (id=35) system
/// message only appears after `EFFECT_LEAVEGAME::onEffectGain` fires
/// (see `vendor/server/scripts/effects/leavegame.lua:35`), and never at
/// all when the request is silently rejected by the 0x0E7 validator
/// (`InEvent | AbnormalStatus | Crafting | PreventAction`) or when a
/// GM / Mog-House grant skips the countdown and disconnects directly.
/// Without a local echo the operator gets zero feedback that their slash
/// was even parsed, which is the bug this addresses.
/// If `cmd` is an *arming* `ReqLogout` variant, return `Some(shutdown)`
/// — the boolean that distinguishes `/logout` from `/shutdown` for the
/// HUD label. `Off` variants and non-`ReqLogout` commands return `None`
/// so the dispatcher doesn't fire a spurious optimistic countdown when
/// the user is *cancelling* a logout (which the existing widget should
/// hide, not show).
fn reqlogout_starts_countdown(cmd: &AgentCommand) -> Option<bool> {
    let AgentCommand::ReqLogout { kind } = cmd else {
        return None;
    };
    match kind {
        ReqLogoutKind::LogoutToggle | ReqLogoutKind::LogoutOn => Some(false),
        ReqLogoutKind::ShutdownToggle | ReqLogoutKind::ShutdownOn => Some(true),
        ReqLogoutKind::LogoutOff | ReqLogoutKind::ShutdownOff => None,
    }
}

fn reqlogout_ack_text(cmd: &AgentCommand) -> Option<&'static str> {
    let AgentCommand::ReqLogout { kind } = cmd else {
        return None;
    };
    Some(match kind {
        ReqLogoutKind::LogoutToggle | ReqLogoutKind::LogoutOn => {
            "/logout: requested (30s LeaveGame timer; movement or `/logout off` cancels)"
        }
        ReqLogoutKind::LogoutOff => "/logout: cancel requested",
        ReqLogoutKind::ShutdownToggle | ReqLogoutKind::ShutdownOn => {
            "/shutdown: requested (30s LeaveGame timer; movement or `/shutdown off` cancels)"
        }
        ReqLogoutKind::ShutdownOff => "/shutdown: cancel requested",
    })
}

/// Local chat-line echo for `/say`, `/sh`, `/p`, `/l`, `/y` etc. The
/// server eventually echoes a 0x017 with the same text, but local echo
/// gives the operator immediate confirmation that their input was
/// captured — and survives the case where the wire round-trip is
/// silently failing.
///
/// The sender label is the player's own character name (from
/// `snapshot.char_name`) so local echo matches the format SE's
/// default client uses for the player's own lines. Falls back to
/// `"you"` only for the rare frame between login and the lobby's
/// char-name resolve.
fn push_local_chat_line(scene_state: &mut SceneState, kind: u8, text: String) {
    use ffxi_viewer_wire::{ChatChannel, ChatLine};
    let channel = match kind {
        0 => ChatChannel::Say,
        1 => ChatChannel::Shout,
        4 => ChatChannel::Party,
        5 => ChatChannel::Linkshell,
        0x1A => ChatChannel::Yell,
        _ => ChatChannel::Other,
    };
    let sender = scene_state
        .snapshot
        .char_name
        .clone()
        .unwrap_or_else(|| "you".into());
    scene_state.push_local_toast(ChatLine {
        channel,
        sender,
        text,
        server_ts: 0,
        local_seq: 0,
    });
}

/// Outbound `/tell` echo. The chat panel formats `Tell` as
/// `>>{sender} : {text}`; passing the *recipient* as `sender` here
/// produces `>>Daisy : msg`, which is the shape SE's client uses for
/// outbound tells. (Inbound tells from the server land via 0x017 with
/// the actual sender's name, hitting the same format.)
fn push_local_tell_echo(scene_state: &mut SceneState, to: String, text: String) {
    use ffxi_viewer_wire::{ChatChannel, ChatLine};
    scene_state.push_local_toast(ChatLine {
        channel: ChatChannel::Tell,
        sender: to,
        text,
        server_ts: 0,
        local_seq: 0,
    });
}

/// What pressing Enter on a given menu label should do. Pure decision
/// surface, parallel to [`QuickActionDispatch`] — the caller performs the
/// side effects (dispatch command, push toast, push/pop a menu frame,
/// transition mode). Stub entries land in `NotImplemented`.
#[derive(Debug, Clone, PartialEq)]
enum MenuDispatch {
    /// Issue a wire command and exit the menu, plus surface this toast
    /// so the operator sees what was dispatched (logout has up to a
    /// ~30s server timer for normal players — the toast is the only
    /// visible feedback we currently have for that window, since we
    /// don't decode the server's countdown messages on opcode 0x053
    /// `SYSTEMMES` yet).
    CommandWithToast { cmd: AgentCommand, toast: String },
    /// Push a submenu frame onto the menu stack and stay in
    /// `InputMode::Menu`. The cursor on the new frame starts at 0.
    /// Caller (handle_menu_key) is responsible for the actual `push`.
    OpenSubmenu(MenuKind),
    /// Apply a keybind change (preset switch, reset, or list). Reuses
    /// the same `apply_keybind_update` helper that powers `/keybinds`,
    /// so the menu and slash-command paths stay in lockstep on
    /// persistence + toast format. List stays in the menu (so the
    /// operator can flip presets after reading it); Preset/Reset exit
    /// to World once applied.
    KeybindUpdate(KeybindUpdate),
    /// Entry isn't wired yet — emit `[menu] {label} — not implemented`
    /// and stay in the menu so the operator can pick something else.
    NotImplemented(String),
}

fn resolve_menu_entry(kind: MenuKind, label: &str) -> MenuDispatch {
    match (kind, label) {
        // Logout: toggle the in-world LeaveGame timer. Choose toggle
        // (rather than always-arm) so a second press of `-` → ↓ ↓ ... →
        // Enter cancels an accidentally-armed logout — same forgiveness
        // retail's confirm dialog provides. The slash form `/logout off`
        // also cancels, which we mention in the toast. Timing reminder:
        // ≈30s for normal players, immediate for GMs / Mog House
        // (`scripts/effects/leavegame.lua::onEffectGain`).
        (MenuKind::Root, "Logout") => MenuDispatch::CommandWithToast {
            cmd: AgentCommand::ReqLogout {
                kind: ReqLogoutKind::LogoutToggle,
            },
            toast: "[menu] Logout requested (~30s; immediate for GMs / \
                    in Mog House). Toggle again or `/logout off` to cancel."
                .into(),
        },
        // Config: open the keybind submenu. Real preset-switching is on
        // the next level; this entry just navigates.
        (MenuKind::Root, "Config") => MenuDispatch::OpenSubmenu(MenuKind::Config),
        // Config submenu entries → keybind updates. Labels match
        // `CONFIG_ENTRIES` in `hud/menu.rs`; keep them in sync.
        (MenuKind::Config, "Standard") => {
            MenuDispatch::KeybindUpdate(KeybindUpdate::Preset(Preset::Standard))
        }
        (MenuKind::Config, "Compact 1") => {
            MenuDispatch::KeybindUpdate(KeybindUpdate::Preset(Preset::Compact1))
        }
        (MenuKind::Config, "Compact 2") => {
            MenuDispatch::KeybindUpdate(KeybindUpdate::Preset(Preset::Compact2))
        }
        (MenuKind::Config, "Reset to defaults") => {
            MenuDispatch::KeybindUpdate(KeybindUpdate::Reset)
        }
        (MenuKind::Config, "Show current bindings") => {
            MenuDispatch::KeybindUpdate(KeybindUpdate::List)
        }
        (_, other) => MenuDispatch::NotImplemented(other.to_string()),
    }
}

/// Menu-mode keystroke handler. Returns `Some(new_mode)` to transition
/// out of the menu (Esc on root → back to World, Enter on a wired entry
/// → back to World); `None` to stay. The current frame's [`MenuKind`]
/// drives both the cursor bounds and the per-entry dispatch — Root and
/// Config submenu share this handler.
///
/// `bindings` is `&mut` (not `&`) because the Config submenu can swap it
/// wholesale via `apply_keybind_update`. The `matches_logical` reads
/// reborrow as `&Bindings` automatically.
fn handle_menu_key(
    key: &Key,
    bindings: &mut Bindings,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    keybinds_state: &mut KeybindsStateRes,
) -> Option<InputMode> {
    // Capture the active screen + cursor up front. `entry_count` and
    // `entry_label` both consult this — keeps the Root/Config branches
    // out of the per-key paths below.
    let (kind, cursor) = {
        let level = stack.current()?;
        (level.kind, level.cursor)
    };
    let entry_count = ffxi_viewer_core::hud::menu::entry_count(kind);
    if bindings.matches_logical(Action::NavUp, key) {
        // Wrap: top → bottom (matches retail menu nav).
        let level = stack.current_mut()?;
        level.cursor = if cursor == 0 {
            entry_count.saturating_sub(1)
        } else {
            cursor - 1
        };
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        // Wrap: bottom → top.
        let level = stack.current_mut()?;
        let next = cursor + 1;
        level.cursor = if next >= entry_count { 0 } else { next };
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        let label = ffxi_viewer_core::hud::menu::entry_label(kind, cursor);
        return match resolve_menu_entry(kind, label) {
            MenuDispatch::CommandWithToast { cmd, toast } => {
                if let Err(e) = cmd_tx.try_send(cmd) {
                    push_system_chat_line(scene_state, format!("[menu] dispatch dropped: {e}"));
                } else {
                    push_system_chat_line(scene_state, toast);
                }
                Some(InputMode::World)
            }
            MenuDispatch::OpenSubmenu(submenu) => {
                stack.push(submenu);
                None
            }
            MenuDispatch::KeybindUpdate(update) => {
                // List stays in the menu so the operator can flip
                // presets after reading the current map. Preset/Reset
                // exit to World once applied — same shape as the slash
                // path's "fire and forget" UX.
                let stay = matches!(update, KeybindUpdate::List);
                apply_keybind_update(update, bindings, keybinds_state, scene_state);
                if stay {
                    None
                } else {
                    Some(InputMode::World)
                }
            }
            MenuDispatch::NotImplemented(label) => {
                push_system_chat_line(scene_state, format!("[menu] {label} — not implemented"));
                None
            }
        };
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        return if !stack.pop() {
            Some(InputMode::World)
        } else {
            None
        };
    }
    None
}

/// Dialog-mode keystroke handler. Up/Down moves the choice cursor;
/// Enter dispatches `EndEventChoice` with the cursor as `EndPara`; Esc
/// dispatches the legacy `EndEvent` (choice 0). Returns `None` to stay
/// in dialog (the `dialog_mode_sync_system` will pop us back to `World`
/// once the server confirms via `EventEnded` and clears the snapshot
/// dialog).
fn handle_dialog_key(
    key: &Key,
    bindings: &Bindings,
    cursor: &mut DialogCursor,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    if bindings.matches_logical(Action::NavUp, key) {
        if cursor.cursor > 0 {
            cursor.cursor -= 1;
        }
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        if cursor.cursor < DIALOG_MAX_CHOICE {
            cursor.cursor += 1;
        }
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        if let Some(d) = scene_state.snapshot.dialog.as_ref() {
            let _ = cmd_tx.try_send(AgentCommand::EndEventChoice {
                event_id: d.npc_id,
                act_index: d.act_index,
                event_num: d.event_num,
                choice: cursor.cursor,
            });
        }
        return None;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        // Legacy "skip" form — same packet the keepalive auto-drain
        // sends. Useful when an event has no meaningful choice and
        // the operator just wants to dismiss it.
        //
        // Optimistically pop to World *and* clear the local dialog
        // snapshot. Without the local clear, `dialog_mode_sync_system`
        // (which runs first next frame) would see `snapshot.dialog =
        // Some(_)` and yank us back into Dialog before the operator
        // could press Enter to interact with their next target. The
        // next ingest from the server replaces this snapshot
        // wholesale, so the override only persists until the server
        // confirms the EndEvent.
        let _ = cmd_tx.try_send(AgentCommand::EndEvent);
        scene_state.snapshot.dialog = None;
        return Some(InputMode::World);
    }
    None
}

/// What pressing Enter on a given QuickAction label should do.
///
/// Pure decision surface: the label string + the current target (if
/// any) decide between dispatching a wire command, surfacing a system
/// chat line (e.g. "no target"), or admitting the entry is a stub. The
/// caller (the Bevy system) is responsible for actually sending the
/// command / pushing the toast.
#[derive(Debug, Clone, PartialEq)]
enum QuickActionDispatch {
    Command(AgentCommand),
    SystemMessage(String),
    NotImplemented(String),
}

fn resolve_quick_action(
    label: &str,
    target: Option<&ffxi_viewer_wire::Entity>,
) -> QuickActionDispatch {
    match label {
        // /check parity: same wire dispatch the slash-command path uses.
        // CheckKind::Check is the basic check (vs. CheckName/CheckParam,
        // which are author-debug variants only reachable via the slash
        // form). The contextual menu only exposes the player-visible one.
        "Check" => match target {
            Some(ent) => QuickActionDispatch::Command(AgentCommand::CheckTarget {
                target_id: ent.id,
                target_index: ent.act_index,
                kind: CheckKind::Check,
            }),
            None => QuickActionDispatch::SystemMessage("[quick] Check: no target".into()),
        },
        // /attack parity. ActionKind::Attack (opcode 0x02) engages the
        // target. The slash-command path sends the same wire shape.
        "Attack" => match target {
            Some(ent) => QuickActionDispatch::Command(AgentCommand::Action {
                target_id: ent.id,
                target_index: ent.act_index,
                kind: ActionKind::Attack,
            }),
            None => QuickActionDispatch::SystemMessage("[quick] Attack: no target".into()),
        },
        // Talk: opcode 0x00. The server treats it as "engage NPC
        // dialogue / get out of the way of an existing event". For
        // mobs/PCs the packet is a no-op; the harm of accidentally
        // firing it is low.
        "Talk" => match target {
            Some(ent) => QuickActionDispatch::Command(AgentCommand::Action {
                target_id: ent.id,
                target_index: ent.act_index,
                kind: ActionKind::Talk,
            }),
            None => QuickActionDispatch::SystemMessage("[quick] Talk: no target".into()),
        },
        // Everything else is still a stub. As more entries get wired,
        // add their label match arms above this fall-through.
        other => QuickActionDispatch::NotImplemented(other.to_string()),
    }
}

fn handle_quick_action_key(
    key: &Key,
    bindings: &Bindings,
    state: &mut QuickActionState,
    scene_state: &mut SceneState,
    target_id: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let entry_count = ffxi_viewer_core::hud::quick_action::entry_count(state.has_target);
    if bindings.matches_logical(Action::NavUp, key) {
        // Wrap: top → bottom. Matches retail's action ring, where
        // there's no "dead" stop at either end of a 3-entry list.
        state.cursor = if state.cursor == 0 {
            entry_count.saturating_sub(1)
        } else {
            state.cursor - 1
        };
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        // Wrap: bottom → top.
        let next = state.cursor + 1;
        state.cursor = if next >= entry_count { 0 } else { next };
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        let label =
            ffxi_viewer_core::hud::quick_action::entry_label(state.has_target, state.cursor);
        let target_ent = target_id.and_then(|id| entities.iter().find(|e| e.id == id));
        match resolve_quick_action(label, target_ent) {
            QuickActionDispatch::Command(cmd) => {
                if let Err(e) = cmd_tx.try_send(cmd) {
                    push_system_chat_line(scene_state, format!("[quick] dispatch dropped: {e}"));
                }
            }
            QuickActionDispatch::SystemMessage(msg) => {
                push_system_chat_line(scene_state, msg);
            }
            QuickActionDispatch::NotImplemented(label) => {
                push_system_chat_line(scene_state, format!("[quick] {label} — not implemented"));
            }
        }
        return Some(InputMode::World);
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        return Some(InputMode::World);
    }
    None
}

/// Passive-cursor mode: arrow keys scroll the focused HUD panel; Esc or
/// the toggle key returns to World. The scroll is in row units (one
/// ChatLine per tick), so the math is independent of any wrapping
/// done at render time.
///
/// `chat_scroll` is the shared resource — also driven by the mouse-wheel
/// system in `hud::chat_panel`, so keyboard and wheel never disagree.
fn handle_passive_cursor_key(
    key: &Key,
    bindings: &Bindings,
    chat_scroll: &mut ChatScroll,
    scene_state: &SceneState,
) -> Option<InputMode> {
    // Number of available rows we can scroll back through, clamped at
    // the oldest line. Recomputed each keypress because new chat
    // arrivals shift the available range.
    let max_back = ffxi_viewer_core::snapshot::rendered_chat(scene_state).len();

    if bindings.matches_logical(Action::NavUp, key) {
        // Scroll one older line into view (saturating at the oldest).
        if chat_scroll.rows + 1 < max_back {
            chat_scroll.rows += 1;
        }
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        // Scroll one newer line into view (saturating at the newest).
        chat_scroll.rows = chat_scroll.rows.saturating_sub(1);
        return None;
    }
    if bindings.matches_logical(Action::PageUp, key) {
        // 8 = chat_panel::VISIBLE_ROWS; one full page back, clamped.
        let next = chat_scroll.rows.saturating_add(8);
        chat_scroll.rows = next.min(max_back.saturating_sub(1));
        return None;
    }
    if bindings.matches_logical(Action::PageDown, key) {
        chat_scroll.rows = chat_scroll.rows.saturating_sub(8);
        return None;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        return Some(InputMode::World);
    }
    // TogglePassiveCursor goes through the physical-key path in
    // input.rs (since it can't be a Nav* shared action — it has its
    // own Action variant). The toggle handler there detects mode ==
    // PassiveCursor and pops back to World.
    None
}

#[cfg(test)]
mod reqlogout_ack_tests {
    use super::*;

    /// Every `ReqLogout` variant must produce a toast. If a future
    /// variant lands without one, the user is back to the
    /// "typed `/logout` and saw nothing" failure mode.
    #[test]
    fn every_reqlogout_kind_has_ack_text() {
        for kind in [
            ReqLogoutKind::LogoutToggle,
            ReqLogoutKind::LogoutOn,
            ReqLogoutKind::LogoutOff,
            ReqLogoutKind::ShutdownToggle,
            ReqLogoutKind::ShutdownOn,
            ReqLogoutKind::ShutdownOff,
        ] {
            let text = reqlogout_ack_text(&AgentCommand::ReqLogout { kind })
                .unwrap_or_else(|| panic!("no toast for {kind:?}"));
            assert!(!text.is_empty(), "empty toast for {kind:?}");
        }
    }

    #[test]
    fn arming_variants_mention_cancellation() {
        for kind in [
            ReqLogoutKind::LogoutToggle,
            ReqLogoutKind::LogoutOn,
            ReqLogoutKind::ShutdownToggle,
            ReqLogoutKind::ShutdownOn,
        ] {
            let text = reqlogout_ack_text(&AgentCommand::ReqLogout { kind })
                .expect("arming variant has ack")
                .to_lowercase();
            assert!(
                text.contains("cancel") || text.contains("off"),
                "{kind:?} toast {text:?} should hint at cancellation",
            );
        }
    }

    #[test]
    fn non_reqlogout_command_returns_none() {
        let other = AgentCommand::Chat {
            kind: 0,
            text: "hi".into(),
        };
        assert!(reqlogout_ack_text(&other).is_none());
    }
}

#[cfg(test)]
mod quick_action_tests {
    use super::*;
    use ffxi_viewer_wire::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};

    fn target_ent(id: u32, act_index: u16) -> WireEntity {
        WireEntity {
            id,
            act_index,
            kind: EntityKind::Mob,
            name: None,
            pos: WireVec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            heading: 0,
            hp_pct: None,
            bt_target_id: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
        }
    }

    #[test]
    fn check_dispatches_check_target_with_basic_kind() {
        let ent = target_ent(0x1234, 7);
        let result = resolve_quick_action("Check", Some(&ent));
        match result {
            QuickActionDispatch::Command(AgentCommand::CheckTarget {
                target_id,
                target_index,
                kind,
            }) => {
                assert_eq!(target_id, 0x1234);
                assert_eq!(target_index, 7);
                assert_eq!(kind, CheckKind::Check);
            }
            other => panic!("expected CheckTarget command, got {other:?}"),
        }
    }

    #[test]
    fn check_with_no_target_returns_system_message() {
        let result = resolve_quick_action("Check", None);
        match result {
            QuickActionDispatch::SystemMessage(msg) => {
                assert!(msg.to_lowercase().contains("no target"));
            }
            other => panic!("expected SystemMessage, got {other:?}"),
        }
    }

    #[test]
    fn unwired_entry_stays_not_implemented() {
        let ent = target_ent(1, 1);
        let result = resolve_quick_action("Magic", Some(&ent));
        assert_eq!(result, QuickActionDispatch::NotImplemented("Magic".into()),);
    }
}

#[cfg(test)]
mod menu_dispatch_tests {
    use super::*;

    #[test]
    fn logout_dispatches_reqlogout_with_toast() {
        match resolve_menu_entry(MenuKind::Root, "Logout") {
            MenuDispatch::CommandWithToast { cmd, toast } => {
                assert_eq!(
                    cmd,
                    AgentCommand::ReqLogout {
                        kind: ReqLogoutKind::LogoutToggle,
                    }
                );
                assert!(
                    toast.to_lowercase().contains("logout"),
                    "toast should mention logout, got {toast:?}"
                );
            }
            other => panic!("expected CommandWithToast for Logout, got {other:?}"),
        }
    }

    #[test]
    fn unwired_root_entries_stay_not_implemented() {
        // `Config` was wired up as a submenu in commit c4a9321 (preset
        // switcher + `/keybinds list`); the test was not updated at
        // the time. The remaining root entries below are still stubs.
        for label in [
            "Magic",
            "Abilities",
            "Items",
            "Status",
            "Party",
            "Search",
            "Macros",
        ] {
            assert_eq!(
                resolve_menu_entry(MenuKind::Root, label),
                MenuDispatch::NotImplemented(label.into()),
                "{label} should still be a stub"
            );
        }
    }
}
