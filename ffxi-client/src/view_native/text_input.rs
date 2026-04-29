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
    ChatBuffer, InputMode, MenuStack, QuickActionState, SceneState, Target, CHAT_HISTORY_CAP,
};
use tokio::sync::mpsc::Sender;

use crate::state::{ActionKind, AgentCommand};
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
                    &cmd_tx.0,
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
                );
            }
            InputMode::Menu(stack) => {
                if let Some(next) = handle_menu_key(&ev.logical_key, stack, &mut scene_state) {
                    *mode = next;
                }
            }
            InputMode::QuickAction(qa) => {
                if let Some(next) = handle_quick_action_key(&ev.logical_key, qa, &mut scene_state) {
                    *mode = next;
                }
            }
        }
    }
}

/// World-mode triggers. Returns `Some(new_mode)` to transition; `None` to
/// stay in world. Side effect: dispatches `Action::Talk` directly on
/// Enter when a target is selected.
fn handle_world_key(
    key: &Key,
    current_target: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    cmd_tx: &Sender<AgentCommand>,
    _target: &mut Target,
) -> Option<InputMode> {
    match key {
        // Space opens chat with an empty buffer. The space itself is
        // consumed (don't seed the buffer — would just be a leading
        // whitespace that's awkward to delete).
        Key::Space => Some(InputMode::Chat(ChatBuffer::empty())),
        // `/` opens chat with the slash already in the buffer so the
        // user sees they're entering a command.
        Key::Character(s) if s.as_str() == "/" => Some(InputMode::Chat(ChatBuffer::with_prefix("/"))),
        // `-` opens the main menu.
        Key::Character(s) if s.as_str() == "-" => Some(InputMode::Menu(MenuStack::root())),
        // Enter: interact with current target, or open the quick-action
        // picker if there's no target. ActionKind::Talk maps to opcode
        // 0x00, which the server treats as "engage NPC dialogue / get
        // out of the way of an existing event". For mobs/PCs the same
        // packet is a no-op; the harm of accidentally sending it is low.
        Key::Enter => match current_target {
            Some(id) => {
                if let Some(ent) = entities.iter().find(|e| e.id == id) {
                    let _ = cmd_tx.try_send(AgentCommand::Action {
                        target_id: ent.id,
                        target_index: ent.act_index,
                        kind: ActionKind::Talk,
                    });
                }
                None
            }
            None => Some(InputMode::QuickAction(QuickActionState::default())),
        },
        _ => None,
    }
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
                apply_slash_outcome(outcome, target, cmd_tx, scene_state, exit);
            } else {
                // Default channel: Say.
                let _ = cmd_tx.try_send(AgentCommand::Chat {
                    kind: 0,
                    text: trimmed.to_string(),
                });
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
) {
    match outcome {
        SlashOutcome::Command(cmd) => {
            let _ = cmd_tx.try_send(cmd);
        }
        SlashOutcome::SetTarget(id) => {
            target.id = id;
        }
        SlashOutcome::Quit => {
            let _ = cmd_tx.try_send(AgentCommand::Disconnect);
            exit.write_default();
        }
        SlashOutcome::SystemMessage(text) => {
            push_system_chat_line(scene_state, text);
        }
    }
}

/// Append a `[system]` chat line to the in-process snapshot so the chat
/// panel renders it. Trims to `CHAT_HISTORY_CAP` so we match the cap
/// the producer uses; the next server-pushed snapshot will overwrite
/// any local additions, which is fine for transient toasts like
/// "unknown command".
fn push_system_chat_line(scene_state: &mut SceneState, text: String) {
    scene_state.snapshot.chat.push(system_chat_line(text));
    let len = scene_state.snapshot.chat.len();
    if len > CHAT_HISTORY_CAP {
        let drop_n = len - CHAT_HISTORY_CAP;
        scene_state.snapshot.chat.drain(0..drop_n);
    }
    // Mark dirty so the chat-panel HUD re-renders this tick.
    scene_state.dirty = true;
}

/// Menu-mode keystroke handler. Returns `Some(new_mode)` to transition
/// out of the menu (Esc on root → back to World); `None` to stay.
fn handle_menu_key(
    key: &Key,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
) -> Option<InputMode> {
    let level = stack.current_mut()?;
    let entry_count = ffxi_viewer_core::hud::menu::root_entry_count();
    match key {
        Key::ArrowUp => {
            if level.cursor > 0 {
                level.cursor -= 1;
            }
            None
        }
        Key::ArrowDown => {
            if level.cursor + 1 < entry_count {
                level.cursor += 1;
            }
            None
        }
        Key::Enter => {
            let label = ffxi_viewer_core::hud::menu::root_entry_label(level.cursor);
            push_system_chat_line(scene_state, format!("[menu] {label} — not implemented"));
            None
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

fn handle_quick_action_key(
    key: &Key,
    state: &mut QuickActionState,
    scene_state: &mut SceneState,
) -> Option<InputMode> {
    let entry_count = ffxi_viewer_core::hud::quick_action::entry_count();
    match key {
        Key::ArrowUp => {
            if state.cursor > 0 {
                state.cursor -= 1;
            }
            None
        }
        Key::ArrowDown => {
            if state.cursor + 1 < entry_count {
                state.cursor += 1;
            }
            None
        }
        Key::Enter => {
            let label = ffxi_viewer_core::hud::quick_action::entry_label(state.cursor);
            push_system_chat_line(scene_state, format!("[quick] {label} — not implemented"));
            Some(InputMode::World)
        }
        Key::Escape => Some(InputMode::World),
        _ => None,
    }
}
