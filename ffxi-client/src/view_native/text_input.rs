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

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use ffxi_viewer_core::{
    ChatBuffer, DialogCursor, InputMode, MenuStack, QuickActionState, SceneState, Target,
    DIALOG_MAX_CHOICE,
};
use tokio::sync::mpsc::Sender;

use crate::state::{ActionKind, AgentCommand, CheckKind, ReqLogoutKind};
use crate::view_native::input::CommandTx;
use crate::view_native::slash_commands::{parse_slash, system_chat_line, SlashOutcome};

/// Read `KeyboardInput` events and route per [`InputMode`]. Runs every
/// `Update` tick. Cmd+Q / window-close are handled in `input.rs`'s
/// `handle_input_system` — keeping them there means quitting works even
/// while a UI is focused.
pub fn text_input_system(
    mut events: MessageReader<KeyboardInput>,
    cmd_tx: Res<CommandTx>,
    mut mode: ResMut<InputMode>,
    mut target: ResMut<Target>,
    mut scene_state: ResMut<SceneState>,
    mut exit: MessageWriter<AppExit>,
    mut navmesh_visible: ResMut<super::navmesh_overlay::NavmeshOverlayVisible>,
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
                if let Some(next) = handle_world_key(
                    &ev.logical_key,
                    current_target,
                    &entities,
                    self_pos,
                    &mut target,
                ) {
                    *mode = next;
                }
            }
            InputMode::Chat(buffer) => {
                let action = handle_chat_key(&ev.logical_key, buffer);
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
                );
            }
            InputMode::Menu(stack) => {
                if let Some(next) =
                    handle_menu_key(&ev.logical_key, stack, &mut scene_state, &cmd_tx.0)
                {
                    *mode = next;
                }
            }
            InputMode::QuickAction(qa) => {
                if let Some(next) = handle_quick_action_key(
                    &ev.logical_key,
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
                    cursor,
                    &mut scene_state,
                    &cmd_tx.0,
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
    current_target: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    self_pos: ffxi_viewer_wire::Vec3,
    target: &mut Target,
) -> Option<InputMode> {
    match key {
        // Space opens chat with an empty buffer. The space itself is
        // consumed (don't seed the buffer — would just be a leading
        // whitespace that's awkward to delete).
        Key::Space => Some(InputMode::Chat(ChatBuffer::empty())),
        // NOTE: `/` (OpenChatCommand) and `-` (OpenMenu) used to live
        // here as logical-key matches against `Key::Character("/")` and
        // `Key::Character("-")`. They moved to `input.rs`'s
        // physical-key reader (`bindings.just_pressed(Action::OpenChat
        // Command|OpenMenu)`) because matching on the *character* is
        // keyboard-layout-fragile — on AZERTY, the physical key under
        // `/` produces `:`, etc. The triggering KeyboardInput event
        // still flows through this system after the mode change; for
        // `/` it lands in handle_chat_key, which appends `/` to the
        // (now Chat) buffer — same final state as the old
        // `with_prefix("/")` shortcut.
        // Enter:
        // - With a current target → open the contextual menu (FFXI-retail:
        //   Enter on a target brings up the action ring, not a direct
        //   Talk). The operator picks Attack/Check/Talk/etc from there.
        // - With no target + nearby NPC → auto-acquire the NPC (no menu
        //   yet). The next Enter press will then open the menu against
        //   that target. This matches the retail "step up to NPC and
        //   press Enter" muscle-memory.
        // - With no target + no nearby NPC → open the menu directly.
        Key::Enter => match current_target {
            Some(_) => Some(InputMode::QuickAction(QuickActionState::for_target(true))),
            None => match nearest_npc(entities, self_pos, AUTO_TARGET_RADIUS) {
                Some(ent) => {
                    target.id = Some(ent.id);
                    None
                }
                None => Some(InputMode::QuickAction(QuickActionState::for_target(false))),
            },
        },
        _ => None,
    }
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

fn handle_chat_key(key: &Key, buffer: &mut ChatBuffer) -> ChatAction {
    match key {
        Key::Enter => ChatAction::Submit,
        Key::Escape => {
            if buffer.text.is_empty() {
                ChatAction::Exit
            } else {
                buffer.text.clear();
                ChatAction::Stay
            }
        }
        Key::Backspace => {
            buffer.text.pop();
            ChatAction::Stay
        }
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
                let outcome = parse_slash(trimmed, entities, self_pos, current_target);
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
                apply_slash_outcome(outcome, target, cmd_tx, scene_state, exit, navmesh_visible);
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

fn apply_slash_outcome(
    outcome: SlashOutcome,
    target: &mut Target,
    cmd_tx: &Sender<AgentCommand>,
    scene_state: &mut SceneState,
    exit: &mut MessageWriter<AppExit>,
    navmesh_visible: &mut super::navmesh_overlay::NavmeshOverlayVisible,
) {
    match outcome {
        SlashOutcome::Command(cmd) => {
            let send_result = cmd_tx.try_send(cmd);
            if let Err(e) = send_result {
                // Channel full or closed — operator should see this; silent
                // drops are how slash bugs hide. The text goes straight to
                // the chat panel via the same path /say already uses.
                push_system_chat_line(
                    scene_state,
                    format!("command dropped (channel issue): {e}"),
                );
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
            let _ = cmd_tx.try_send(AgentCommand::ReqLogout { kind });
            let _ = cmd_tx.try_send(AgentCommand::Disconnect);
            exit.write_default();
        }
        SlashOutcome::SystemMessage(text) => {
            push_system_chat_line(scene_state, text);
        }
        SlashOutcome::ToggleNavmesh(setting) => {
            let next = setting.unwrap_or(!navmesh_visible.0);
            navmesh_visible.0 = next;
            let label = if next { "ON" } else { "OFF" };
            push_system_chat_line(scene_state, format!("navmesh overlay: {label}"));
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
    });
}

/// What pressing Enter on a given root-menu label should do. Pure
/// decision surface, parallel to [`QuickActionDispatch`] — the caller
/// performs the side effects (dispatch command, push toast, transition
/// mode). Stub entries land in `NotImplemented`. As more entries get
/// wired (and some need silent dispatch instead of a toast), add a
/// `Command(AgentCommand)` variant alongside `CommandWithToast`.
#[derive(Debug, Clone, PartialEq)]
enum MenuDispatch {
    /// Issue a wire command and exit the menu, plus surface this toast
    /// so the operator sees what was dispatched (logout has up to a
    /// ~30s server timer for normal players — the toast is the only
    /// visible feedback we currently have for that window, since we
    /// don't decode the server's countdown messages on opcode 0x053
    /// `SYSTEMMES` yet).
    CommandWithToast {
        cmd: AgentCommand,
        toast: String,
    },
    /// Entry isn't wired yet — emit `[menu] {label} — not implemented`
    /// and stay in the menu so the operator can pick something else.
    NotImplemented(String),
}

fn resolve_menu_entry(label: &str) -> MenuDispatch {
    match label {
        // Logout: toggle the in-world LeaveGame timer. Choose toggle
        // (rather than always-arm) so a second press of `-` → ↓ ↓ ... →
        // Enter cancels an accidentally-armed logout — same forgiveness
        // retail's confirm dialog provides. The slash form `/logout off`
        // also cancels, which we mention in the toast. Timing reminder:
        // ≈30s for normal players, immediate for GMs / Mog House
        // (`scripts/effects/leavegame.lua::onEffectGain`).
        "Logout" => MenuDispatch::CommandWithToast {
            cmd: AgentCommand::ReqLogout {
                kind: ReqLogoutKind::LogoutToggle,
            },
            toast: "[menu] Logout requested (~30s; immediate for GMs / \
                    in Mog House). Toggle again or `/logout off` to cancel."
                .into(),
        },
        other => MenuDispatch::NotImplemented(other.to_string()),
    }
}

/// Menu-mode keystroke handler. Returns `Some(new_mode)` to transition
/// out of the menu (Esc on root → back to World, Enter on a wired entry
/// → back to World); `None` to stay.
fn handle_menu_key(
    key: &Key,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let level = stack.current_mut()?;
    let entry_count = ffxi_viewer_core::hud::menu::root_entry_count();
    match key {
        Key::ArrowUp => {
            // Wrap: top → bottom (matches retail menu nav).
            level.cursor = if level.cursor == 0 {
                entry_count.saturating_sub(1)
            } else {
                level.cursor - 1
            };
            None
        }
        Key::ArrowDown => {
            // Wrap: bottom → top.
            let next = level.cursor + 1;
            level.cursor = if next >= entry_count { 0 } else { next };
            None
        }
        Key::Enter => {
            let label = ffxi_viewer_core::hud::menu::root_entry_label(level.cursor);
            match resolve_menu_entry(label) {
                MenuDispatch::CommandWithToast { cmd, toast } => {
                    if let Err(e) = cmd_tx.try_send(cmd) {
                        push_system_chat_line(
                            scene_state,
                            format!("[menu] dispatch dropped: {e}"),
                        );
                    } else {
                        push_system_chat_line(scene_state, toast);
                    }
                    Some(InputMode::World)
                }
                MenuDispatch::NotImplemented(label) => {
                    push_system_chat_line(
                        scene_state,
                        format!("[menu] {label} — not implemented"),
                    );
                    None
                }
            }
        }
        Key::Escape => {
            if !stack.pop() {
                Some(InputMode::World)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Dialog-mode keystroke handler. Up/Down moves the choice cursor;
/// Enter dispatches `EndEventChoice` with the cursor as `EndPara`; Esc
/// dispatches the legacy `EndEvent` (choice 0). Returns `None` to stay
/// in dialog (the `dialog_mode_sync_system` will pop us back to `World`
/// once the server confirms via `EventEnded` and clears the snapshot
/// dialog).
fn handle_dialog_key(
    key: &Key,
    cursor: &mut DialogCursor,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    match key {
        Key::ArrowUp => {
            if cursor.cursor > 0 {
                cursor.cursor -= 1;
            }
            None
        }
        Key::ArrowDown => {
            if cursor.cursor < DIALOG_MAX_CHOICE {
                cursor.cursor += 1;
            }
            None
        }
        Key::Enter => {
            if let Some(d) = scene_state.snapshot.dialog.as_ref() {
                let _ = cmd_tx.try_send(AgentCommand::EndEventChoice {
                    event_id: d.npc_id,
                    act_index: d.act_index,
                    event_num: d.event_num,
                    choice: cursor.cursor,
                });
            }
            None
        }
        Key::Escape => {
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
            Some(InputMode::World)
        }
        _ => None,
    }
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
    state: &mut QuickActionState,
    scene_state: &mut SceneState,
    target_id: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let entry_count = ffxi_viewer_core::hud::quick_action::entry_count(state.has_target);
    match key {
        Key::ArrowUp => {
            // Wrap: top → bottom. Matches retail's action ring, where
            // there's no "dead" stop at either end of a 3-entry list.
            state.cursor = if state.cursor == 0 {
                entry_count.saturating_sub(1)
            } else {
                state.cursor - 1
            };
            None
        }
        Key::ArrowDown => {
            // Wrap: bottom → top.
            let next = state.cursor + 1;
            state.cursor = if next >= entry_count { 0 } else { next };
            None
        }
        Key::Enter => {
            let label = ffxi_viewer_core::hud::quick_action::entry_label(
                state.has_target,
                state.cursor,
            );
            let target_ent = target_id.and_then(|id| entities.iter().find(|e| e.id == id));
            match resolve_quick_action(label, target_ent) {
                QuickActionDispatch::Command(cmd) => {
                    if let Err(e) = cmd_tx.try_send(cmd) {
                        push_system_chat_line(
                            scene_state,
                            format!("[quick] dispatch dropped: {e}"),
                        );
                    }
                }
                QuickActionDispatch::SystemMessage(msg) => {
                    push_system_chat_line(scene_state, msg);
                }
                QuickActionDispatch::NotImplemented(label) => {
                    push_system_chat_line(
                        scene_state,
                        format!("[quick] {label} — not implemented"),
                    );
                }
            }
            Some(InputMode::World)
        }
        Key::Escape => Some(InputMode::World),
        _ => None,
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
            pos: WireVec3 { x: 0.0, y: 0.0, z: 0.0 },
            heading: 0,
            hp_pct: None,
            bt_target_id: 0,
            claim_id: 0,
        }
    }

    #[test]
    fn check_dispatches_check_target_with_basic_kind() {
        let ent = target_ent(0x1234, 7);
        let result = resolve_quick_action("Check", Some(&ent));
        match result {
            QuickActionDispatch::Command(AgentCommand::CheckTarget { target_id, target_index, kind }) => {
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
        assert_eq!(
            result,
            QuickActionDispatch::NotImplemented("Magic".into()),
        );
    }
}

#[cfg(test)]
mod menu_dispatch_tests {
    use super::*;

    #[test]
    fn logout_dispatches_reqlogout_with_toast() {
        match resolve_menu_entry("Logout") {
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
        for label in ["Magic", "Abilities", "Items", "Status", "Party",
                      "Search", "Macros", "Config"] {
            assert_eq!(
                resolve_menu_entry(label),
                MenuDispatch::NotImplemented(label.into()),
                "{label} should still be a stub"
            );
        }
    }
}
