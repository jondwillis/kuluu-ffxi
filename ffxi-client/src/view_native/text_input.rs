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
    QuickActionState, SceneState, Target,
};

use super::debug_heights::DebugHeightsRequest;

#[derive(Resource, Default)]
pub struct CaptureMode {
    pub active: bool,

    pub restore_limiter: Option<bevy_framepace::Limiter>,
}

#[derive(SystemParam)]
pub struct SlashWriters<'w, 's> {
    pub load_mmb: MessageWriter<'w, LoadMmbRequest>,
    pub load_mzb: MessageWriter<'w, LoadMzbRequest>,
    pub debug_heights: MessageWriter<'w, DebugHeightsRequest>,

    pub logout_requested:
        MessageWriter<'w, ffxi_viewer_core::hud::logout_countdown::LogoutRequested>,

    pub framepace: ResMut<'w, bevy_framepace::FramepaceSettings>,

    pub primary_window: Query<'w, 's, &'static mut Window, With<PrimaryWindow>>,

    pub capture_mode: ResMut<'w, CaptureMode>,

    pub event_log: ResMut<'w, ffxi_viewer_core::EventLog>,

    pub sfx_event: MessageWriter<'w, ffxi_viewer_core::audio::SfxEvent>,

    pub screenshot: MessageWriter<'w, super::screenshot::ScreenshotRequest>,

    pub graphics: ResMut<'w, ffxi_viewer_core::GraphicsSettings>,

    pub hud_verbosity: ResMut<'w, ffxi_viewer_core::hud::HudVerbosity>,

    pub net_status_visible: ResMut<'w, ffxi_viewer_core::hud::network_status::NetStatusVisible>,

    pub minimap_mode: ResMut<'w, ffxi_viewer_core::minimap::MinimapMode>,

    pub minimap_visible: ResMut<'w, ffxi_viewer_core::minimap::MinimapVisible>,

    pub topdown_cull: ResMut<'w, ffxi_viewer_core::minimap::topdown::TopdownCullPolicy>,

    pub audio_mute: ResMut<'w, ffxi_viewer_core::audio::AudioMuteState>,

    pub minimap_zoom: ResMut<'w, ffxi_viewer_core::minimap::MinimapZoom>,

    pub minimap_view: ResMut<'w, ffxi_viewer_core::minimap::MinimapView>,

    pub minimap_state: Res<'w, ffxi_viewer_core::minimap::MinimapState>,

    pub rest_stance: ResMut<'w, ffxi_viewer_core::combat_stance::RestStance>,

    pub status_profile_open: ResMut<'w, ffxi_viewer_core::hud::status_panel::StatusProfileOpen>,

    pub check_target: ResMut<'w, ffxi_viewer_core::hud::check_view::CheckTarget>,

    pub trade_state: ResMut<'w, ffxi_viewer_core::hud::trade::TradeState>,

    pub trade_intent: MessageWriter<'w, ffxi_viewer_core::hud::trade::TradeIntent>,

    pub select_target: ResMut<'w, SelectTargetMode>,
}
use tokio::sync::mpsc::Sender;

use crate::keybinds_store::KeybindsStateRes;
use crate::state::{ActionKind, AgentCommand, AgentEvent, CheckKind, ReqLogoutKind};
use crate::view_native::input::{CommandTx, SelectTargetMode};
use crate::view_native::slash_commands::{
    parse_slash, system_chat_line, KeybindUpdate, SlashOutcome,
};

fn minimap_retail_desc(
    state: &ffxi_viewer_core::minimap::MinimapState,
    zone: Option<u16>,
) -> String {
    use ffxi_viewer_core::minimap::RetailStatus;
    let Some(z) = zone else {
        return "no active zone".into();
    };
    if state.retail_zone == Some(z) {
        match &state.retail_status {
            RetailStatus::Loaded => return "loaded".into(),
            RetailStatus::Failed(why) => return format!("unavailable — {why}"),
            RetailStatus::Idle => {}
        }
    }
    match ffxi_dat::map_image::map_dat_for_zone(z) {
        Some(file_id) => format!(
            "pending (zone {z} maps to file {file_id}; img={} rzone={:?} failed={})",
            state.retail_image.is_some(),
            state.retail_zone,
            state.retail_failed_zones.contains(&z),
        ),
        None => format!("no map-DAT mapping for zone {z}"),
    }
}

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

    #[cfg(unix)] agent_paused: Option<Res<super::AgentPaused>>,
    session_event_tx: Option<Res<super::SessionEventTx>>,

    mut slash_writers: SlashWriters,

    mut draw_distance: ResMut<ffxi_viewer_core::dat_mzb::DrawDistance>,

    mut chat_scroll: ResMut<ChatScroll>,

    dynamic_menu: Res<ffxi_viewer_core::hud::menu::DynamicMenu>,
) {
    let entities = scene_state.snapshot.entities.clone();
    let self_pos = scene_state.snapshot.self_pos.pos;
    let current_target = target.id;
    let engaged = matches!(
        scene_state.snapshot.current_goal,
        Some(ffxi_viewer_wire::ReactorGoal::Engaged { .. })
    );

    let target_changed = target.is_changed();

    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &mut *mode {
            InputMode::World => {
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
                if slash_writers.select_target.active {
                    if bindings.matches_logical(Action::ConfirmAction, &ev.logical_key) {
                        if let Some(id) = current_target {
                            let _ = cmd_tx.0.try_send(AgentCommand::Engage { target_id: id });
                        }
                        slash_writers.select_target.active = false;
                        slash_writers.select_target.prev = None;
                        continue;
                    }
                    if bindings.matches_logical(Action::ClearTarget, &ev.logical_key) {
                        target.id = slash_writers.select_target.prev.take();
                        slash_writers.select_target.active = false;
                        continue;
                    }
                }
                if let Some(next) = handle_world_key(
                    &ev.logical_key,
                    &bindings,
                    current_target,
                    &entities,
                    self_pos,
                    scene_state.snapshot.self_char_id,
                    target_changed,
                    engaged,
                    &cmd_tx.0,
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
                    &mut slash_writers.graphics,
                    &mut slash_writers.status_profile_open,
                    &dynamic_menu,
                    current_target,
                    self_pos,
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
            InputMode::TargetAction(state) => {
                if let Some(next) = handle_target_action_key(
                    &ev.logical_key,
                    &bindings,
                    state,
                    &mut scene_state,
                    current_target,
                    &entities,
                    &cmd_tx.0,
                    &mut slash_writers.check_target,
                    &mut slash_writers.trade_state,
                    &mut slash_writers.trade_intent,
                    &mut slash_writers.select_target,
                ) {
                    *mode = next;
                }
            }
        }
    }
}

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

#[allow(clippy::too_many_arguments)]
fn handle_world_key(
    key: &Key,
    bindings: &Bindings,
    current_target: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    self_pos: ffxi_viewer_wire::Vec3,
    self_id: Option<u32>,
    target_changed: bool,
    engaged: bool,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    if bindings.matches_logical(Action::OpenChat, key) {
        return Some(InputMode::Chat(ChatBuffer::empty()));
    }
    if bindings.matches_logical(Action::ConfirmAction, key) {
        return match current_target {
            Some(_) if target_changed => None,
            Some(id) => {
                let ent = entities.iter().find(|e| e.id == id);
                let is_npc = matches!(ent.map(|e| e.kind), Some(ffxi_viewer_wire::EntityKind::Npc));
                let in_range = ent.is_some_and(|e| {
                    let dx = e.pos.x - self_pos.x;
                    let dy = e.pos.y - self_pos.y;
                    let dz = e.pos.z - self_pos.z;
                    let r = ffxi_viewer_core::hud::action_model::NPC_INTERACT_YALMS;
                    dx * dx + dy * dy + dz * dz <= r * r
                });
                if is_npc {
                    if let (true, Some(ent)) = (in_range, ent) {
                        let _ = cmd_tx.try_send(AgentCommand::Action {
                            target_id: ent.id,
                            target_index: ent.act_index,
                            kind: ActionKind::Talk,
                        });
                    }
                    None
                } else {
                    open_target_action_menu(current_target, entities, self_pos, self_id, engaged)
                }
            }
            None => None,
        };
    }
    None
}

fn open_target_action_menu(
    current_target: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    self_pos: ffxi_viewer_wire::Vec3,
    self_id: Option<u32>,
    engaged: bool,
) -> Option<InputMode> {
    use ffxi_viewer_core::hud::action_model;
    let ctx =
        action_model::context_for_target(current_target, entities, self_pos, self_id, engaged);
    if action_model::build_target_action_entries(&ctx, &ffxi_viewer_core::hud::overlay::RETAIL)
        .is_empty()
    {
        return None;
    }
    Some(InputMode::TargetAction(
        ffxi_viewer_core::input_mode::TargetActionState::open(ctx),
    ))
}

#[allow(clippy::too_many_arguments)]
fn handle_target_action_key(
    key: &Key,
    bindings: &Bindings,
    state: &mut ffxi_viewer_core::input_mode::TargetActionState,
    scene_state: &mut SceneState,
    current_target: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    cmd_tx: &Sender<AgentCommand>,
    check_target: &mut ffxi_viewer_core::hud::check_view::CheckTarget,
    trade_state: &mut ffxi_viewer_core::hud::trade::TradeState,
    trade_intent: &mut MessageWriter<ffxi_viewer_core::hud::trade::TradeIntent>,
    select_target: &mut SelectTargetMode,
) -> Option<InputMode> {
    use ffxi_viewer_core::hud::action_model::{ActionEntryKind, TargetActionId};
    use ffxi_viewer_core::input_mode::SubAction;

    if trade_state.open {
        return handle_trade_key(key, bindings, trade_state, trade_intent, scene_state);
    }

    if let Some(SubAction::AbilitiesGroup(group)) = state.sub.as_ref().and_then(|s| s.current()) {
        return handle_abilities_group_key(
            key,
            bindings,
            state,
            group,
            scene_state,
            current_target,
            entities,
            cmd_tx,
        );
    }

    let entries = ffxi_viewer_core::hud::overlay::RETAIL.resolve_target_actions(&state.ctx);
    let count = entries.len();
    if count == 0 {
        return Some(InputMode::World);
    }
    if state.cursor >= count {
        state.cursor = count - 1;
    }

    if bindings.matches_logical(Action::NavUp, key) {
        state.cursor = if state.cursor == 0 {
            count - 1
        } else {
            state.cursor - 1
        };
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        let next = state.cursor + 1;
        state.cursor = if next >= count { 0 } else { next };
        return None;
    }
    if bindings.matches_logical(Action::NavRight, key) {
        if let Some(entry) = entries.get(state.cursor) {
            if let ActionEntryKind::Select { modes, .. } = &entry.kind {
                if !modes.is_empty() {
                    match entry.id {
                        TargetActionId::Chat => {
                            state.chat_mode_idx = (state.chat_mode_idx + 1) % modes.len();
                        }
                        TargetActionId::Abilities => {
                            state.abilities_group_idx =
                                (state.abilities_group_idx + 1) % modes.len();
                        }
                        _ => {}
                    }
                }
            }
        }
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        return confirm_target_action_at_cursor(
            state,
            &entries,
            scene_state,
            current_target,
            entities,
            cmd_tx,
            check_target,
            trade_state,
            select_target,
        );
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        if check_target.open {
            check_target.open = false;
            check_target.target_id = None;
        }
        return Some(InputMode::World);
    }
    None
}

fn handle_trade_key(
    key: &Key,
    bindings: &Bindings,
    trade_state: &mut ffxi_viewer_core::hud::trade::TradeState,
    trade_intent: &mut MessageWriter<ffxi_viewer_core::hud::trade::TradeIntent>,
    scene_state: &mut SceneState,
) -> Option<InputMode> {
    use ffxi_viewer_core::hud::trade::{self, TradeFocus, TradeSelector};

    if let Some(selector) = trade_state.selector.clone() {
        match selector {
            TradeSelector::Gil { .. } => {
                if bindings.matches_logical(Action::NavConfirm, key) {
                    trade::gil_confirm(trade_state);
                    return None;
                }
                if bindings.matches_logical(Action::NavCancel, key) {
                    trade_state.selector = None;
                    return None;
                }

                if matches!(key, Key::Tab) {
                    trade::gil_fill_max(trade_state);
                    return None;
                }

                if let Key::Character(s) = key {
                    for c in s.chars() {
                        trade::gil_push_digit(trade_state, c);
                    }
                }
                return None;
            }
            TradeSelector::Stack { .. } => {
                if bindings.matches_logical(Action::NavConfirm, key) {
                    trade::stack_confirm(trade_state);
                    return None;
                }
                if bindings.matches_logical(Action::NavCancel, key) {
                    trade_state.selector = None;
                    return None;
                }
                if bindings.matches_logical(Action::NavUp, key) {
                    trade::stack_adjust(trade_state, 1);
                    return None;
                }
                if bindings.matches_logical(Action::NavDown, key) {
                    trade::stack_adjust(trade_state, -1);
                    return None;
                }
                if bindings.matches_logical(Action::NavRight, key) {
                    if let Some(TradeSelector::Stack { value, max, .. }) =
                        trade_state.selector.as_mut()
                    {
                        *value = *max;
                    }
                    return None;
                }
                return None;
            }
        }
    }

    if bindings.matches_logical(Action::NavUp, key) {
        trade::focus_up(trade_state);
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        trade::focus_down(trade_state);
        return None;
    }
    if bindings.matches_logical(Action::NavLeft, key) {
        trade::focus_left(trade_state);
        return None;
    }
    if bindings.matches_logical(Action::NavRight, key) {
        trade::focus_right(trade_state);
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        match trade_state.focus {
            TradeFocus::Gil => {
                let snapshot_gil = 0;
                trade::begin_gil_entry(trade_state, snapshot_gil);
                None
            }
            TradeFocus::Slot(_) => {
                push_system_chat_line(
                    scene_state,
                    "[trade] Item placement not wired yet — gil-only for now".into(),
                );
                None
            }
            TradeFocus::Ok => {
                trade_intent.write(trade::TradeIntent::Confirm {
                    target_id: trade_state.target_id,
                });
                push_system_chat_line(
                    scene_state,
                    "[trade] Trade sent (gil only; outbound 0x036 pending consumer)".into(),
                );
                trade_state.reset();
                Some(InputMode::World)
            }
            TradeFocus::Cancel => {
                trade_intent.write(trade::TradeIntent::Cancel);
                trade_state.reset();
                Some(InputMode::World)
            }
        }
    } else if bindings.matches_logical(Action::NavCancel, key) {
        trade_intent.write(trade::TradeIntent::Cancel);
        trade_state.reset();
        Some(InputMode::World)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn confirm_target_action_at_cursor(
    state: &mut ffxi_viewer_core::input_mode::TargetActionState,
    entries: &[ffxi_viewer_core::hud::action_model::ActionEntry],
    scene_state: &mut SceneState,
    current_target: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    cmd_tx: &Sender<AgentCommand>,
    check_target: &mut ffxi_viewer_core::hud::check_view::CheckTarget,
    trade_state: &mut ffxi_viewer_core::hud::trade::TradeState,
    select_target: &mut SelectTargetMode,
) -> Option<InputMode> {
    use ffxi_viewer_core::hud::action_model::TargetActionId;

    let Some(entry) = entries.get(state.cursor) else {
        return Some(InputMode::World);
    };
    if !entry.enabled {
        if let Some(hint) = &entry.hint {
            push_system_chat_line(scene_state, format!("[menu] {hint}"));
        }
        return None;
    }

    let target_ent = current_target.and_then(|id| entities.iter().find(|e| e.id == id));
    match entry.id {
        TargetActionId::Attack => {
            match target_ent {
                Some(e) => {
                    if let Err(err) = cmd_tx.try_send(AgentCommand::Engage { target_id: e.id }) {
                        push_system_chat_line(
                            scene_state,
                            format!("[menu] Attack dispatch dropped: {err}"),
                        );
                    }
                }
                None => push_system_chat_line(scene_state, "[menu] Attack: no target".to_string()),
            }
            Some(InputMode::World)
        }
        TargetActionId::SwitchTarget => {
            select_target.active = true;
            select_target.prev = current_target;
            push_system_chat_line(
                scene_state,
                "[menu] Switch Target — Tab to cycle, Enter to confirm, Esc to cancel".to_string(),
            );
            Some(InputMode::World)
        }
        TargetActionId::Disengage => {
            if let Err(err) = cmd_tx.try_send(AgentCommand::Cancel) {
                push_system_chat_line(
                    scene_state,
                    format!("[menu] Disengage dispatch dropped: {err}"),
                );
            }
            Some(InputMode::World)
        }
        TargetActionId::Chat => Some(InputMode::Chat(chat_buffer_for_mode(
            state.chat_mode_idx,
            target_ent,
        ))),
        TargetActionId::Magic => Some(open_submenu(MenuKind::Magic)),
        TargetActionId::Abilities => {
            use ffxi_viewer_core::hud::action_model::AbilityGroup;
            use ffxi_viewer_core::input_mode::{SubAction, SubActionStack};
            let group = AbilityGroup::ALL[state.abilities_group_idx % AbilityGroup::ALL.len()];
            state.sub = Some(SubActionStack::with(SubAction::AbilitiesGroup(group)));
            None
        }
        TargetActionId::Items => Some(open_submenu(MenuKind::Items)),
        TargetActionId::Check => {
            use ffxi_viewer_core::hud::action_model::TargetKindLite;
            match target_ent {
                Some(e) => {
                    let cmd = AgentCommand::CheckTarget {
                        target_id: e.id,
                        target_index: e.act_index,
                        kind: CheckKind::Check,
                    };
                    if let Err(err) = cmd_tx.try_send(cmd) {
                        push_system_chat_line(
                            scene_state,
                            format!("[menu] Check dispatch dropped: {err}"),
                        );
                    }

                    let is_pc = matches!(
                        state.ctx.target_kind,
                        TargetKindLite::Pc | TargetKindLite::SelfPc
                    );
                    if is_pc {
                        check_target.open = true;
                        check_target.target_id = Some(e.id);
                        None
                    } else {
                        Some(InputMode::World)
                    }
                }
                None => {
                    push_system_chat_line(scene_state, "[menu] Check: no target".into());
                    Some(InputMode::World)
                }
            }
        }
        TargetActionId::Trade => match target_ent {
            Some(e) => {
                *trade_state = ffxi_viewer_core::hud::trade::TradeState::open(e.id);
                None
            }
            None => {
                push_system_chat_line(scene_state, "[menu] Trade: no target".into());
                Some(InputMode::World)
            }
        },
        TargetActionId::Trust => {
            push_system_chat_line(scene_state, "[menu] Trust — not implemented yet".into());
            Some(InputMode::World)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_abilities_group_key(
    key: &Key,
    bindings: &Bindings,
    state: &mut ffxi_viewer_core::input_mode::TargetActionState,
    group: ffxi_viewer_core::hud::action_model::AbilityGroup,
    scene_state: &mut SceneState,
    current_target: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let rows = ffxi_viewer_core::hud::menu::ability_group_rows(&scene_state.snapshot, group);
    let count = rows.len();

    let sub = state.sub.as_mut()?;
    if count > 0 && sub.cursor >= count {
        sub.cursor = count - 1;
    }

    if bindings.matches_logical(Action::NavUp, key) {
        if count > 0 {
            sub.cursor = if sub.cursor == 0 {
                count - 1
            } else {
                sub.cursor - 1
            };
        }
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        if count > 0 {
            let next = sub.cursor + 1;
            sub.cursor = if next >= count { 0 } else { next };
        }
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        if let Some(row) = rows.get(sub.cursor) {
            let self_pos = scene_state.snapshot.self_pos.pos;
            dispatch_dynamic_menu_action(
                row.action,
                current_target,
                self_pos,
                entities,
                cmd_tx,
                scene_state,
            );
            return Some(InputMode::World);
        }

        return None;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        if !sub.pop() {
            state.sub = None;
        }
        return None;
    }
    None
}

fn open_submenu(kind: MenuKind) -> InputMode {
    let mut stack = MenuStack::root();
    stack.push(kind);
    InputMode::Menu(stack)
}

fn chat_buffer_for_mode(
    mode_idx: usize,
    target_ent: Option<&ffxi_viewer_wire::Entity>,
) -> ChatBuffer {
    match mode_idx {
        1 => match target_ent.and_then(|e| e.name.as_deref()) {
            Some(name) => ChatBuffer::with_prefix(&format!("/tell {name} ")),
            None => ChatBuffer::empty(),
        },
        2 => ChatBuffer::with_prefix("/p "),
        3 => ChatBuffer::with_prefix("/l "),
        5 => ChatBuffer::with_prefix("/sh "),
        _ => ChatBuffer::empty(),
    }
}

enum ChatAction {
    Stay,
    Submit,
    Exit,
}

fn handle_chat_key(key: &Key, bindings: &Bindings, buffer: &mut ChatBuffer) -> ChatAction {
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
                    scene_state.snapshot.self_char_id,
                    &scene_state.snapshot.party,
                );
                tracing::debug!(buffer = %trimmed, outcome = ?outcome, "chat submit: slash");

                match &outcome {
                    SlashOutcome::Command(AgentCommand::Chat { kind, text }) => {
                        push_local_chat_line(scene_state, *kind, text.clone());
                    }

                    SlashOutcome::Command(AgentCommand::Tell { to, text }) => {
                        push_local_tell_echo(scene_state, to.clone(), text.clone());
                    }
                    _ => {}
                }

                let mode_override = match &outcome {
                    SlashOutcome::OpenMenu(kind) => {
                        let mut stack = MenuStack::root();
                        stack.push(*kind);
                        Some(InputMode::Menu(stack))
                    }
                    _ => None,
                };
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
                if let Some(next) = mode_override {
                    *mode = next;
                    return;
                }
            } else {
                tracing::debug!(text = %trimmed, "chat submit: say");

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
                slash_writers
                    .logout_requested
                    .write(ffxi_viewer_core::hud::logout_countdown::LogoutRequested { shutdown });
            }
            mirror_heal_stance(&cmd, &mut slash_writers.rest_stance);
            let send_result = cmd_tx.try_send(cmd);
            if let Err(e) = send_result {
                push_system_chat_line(scene_state, format!("command dropped (channel issue): {e}"));
            }
        }
        SlashOutcome::Commands(cmds) => {
            for cmd in cmds {
                if let Some(toast) = reqlogout_ack_text(&cmd) {
                    push_system_chat_line(scene_state, toast.into());
                }
                mirror_heal_stance(&cmd, &mut slash_writers.rest_stance);
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
            super::exit_watchdog::arm();
        }
        SlashOutcome::QuitWithLogout(kind) => {
            let req = AgentCommand::ReqLogout { kind };
            if let Some(toast) = reqlogout_ack_text(&req) {
                push_system_chat_line(scene_state, toast.into());
            }
            if let Some(shutdown) = reqlogout_starts_countdown(&req) {
                slash_writers
                    .logout_requested
                    .write(ffxi_viewer_core::hud::logout_countdown::LogoutRequested { shutdown });
            }
            let _ = cmd_tx.try_send(req);
            let _ = cmd_tx.try_send(AgentCommand::Disconnect);
            exit.write_default();
            super::exit_watchdog::arm();
        }
        SlashOutcome::SystemMessage(text) => {
            for line in text.split('\n') {
                push_system_chat_line(scene_state, line.to_string());
            }
        }
        SlashOutcome::SetWeatherClient(w) => {
            scene_state.snapshot.weather = Some(w);
            push_system_chat_line(scene_state, format!("weather override: {w:?}"));
        }
        SlashOutcome::SetSitStance(toggle) => {
            use crate::view_native::slash_commands::SitToggle;
            use ffxi_viewer_core::combat_stance::RestKind;
            let next = match toggle {
                SitToggle::On => RestKind::Sit,
                SitToggle::Off => RestKind::None,
                SitToggle::Toggle => match slash_writers.rest_stance.kind {
                    RestKind::Sit => RestKind::None,

                    RestKind::Heal => {
                        let _ = cmd_tx.try_send(AgentCommand::Heal {
                            mode: crate::state::HealMode::Off,
                        });
                        RestKind::Sit
                    }
                    RestKind::None => RestKind::Sit,
                },
            };
            slash_writers.rest_stance.kind = next;
            let label = match next {
                RestKind::Sit => "sitting",
                RestKind::Heal => "healing",
                RestKind::None => "standing",
            };
            push_system_chat_line(scene_state, format!("/sit: {label}"));
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
        SlashOutcome::Screenshot { path } => {
            static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let resolved = path.unwrap_or_else(|| {
                let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                format!("screenshot-{n}.png")
            });
            slash_writers
                .screenshot
                .write(super::screenshot::ScreenshotRequest {
                    path: std::path::PathBuf::from(&resolved),
                });
        }
        SlashOutcome::PlayBgm { track_id } => {
            slash_writers
                .event_log
                .recent
                .push_back(ffxi_viewer_wire::ViewerEvent::MusicChanged { slot: 0, track_id });
            push_system_chat_line(scene_state, format!("/bgm {track_id}: queued"));
        }
        SlashOutcome::PlaySfx { se_id } => {
            slash_writers
                .sfx_event
                .write(ffxi_viewer_core::audio::SfxEvent::new(se_id));
            push_system_chat_line(scene_state, format!("/sfx {se_id}: fired"));
        }
        SlashOutcome::EndCutscene { event_num } => {
            let resolved_csid = event_num
                .or_else(|| scene_state.snapshot.dialog.as_ref().map(|d| d.event_para))
                .or_else(|| {
                    scene_state
                        .snapshot
                        .zone_id
                        .and_then(crate::view_native::slash_commands::start_zone_cutscene)
                });
            let Some(csid) = resolved_csid else {
                push_system_chat_line(
                    scene_state,
                    "/endcutscene: no active event and current zone isn't a \
                     starting nation; pass an explicit CSID \
                     (`/endcutscene <csid>`) or use `/release`"
                        .into(),
                );
                return;
            };

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
                            "/endcutscene: sending EVENT_END (csid={csid}, \
                             unique_no=0x{event_id:08X}, act_index={act_index})"
                        ),
                    );
                    if let Err(e) = cmd_tx.try_send(AgentCommand::EndEventChoice {
                        event_id,
                        act_index,
                        event_num: csid,
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
        SlashOutcome::SetCameraCollisionSource(setting) => {
            let next = setting.unwrap_or_else(|| draw_distance.camera_collision_source.cycle());
            draw_distance.camera_collision_source = next;
            push_system_chat_line(scene_state, format!("/zonegeom source: {}", next.label()));
        }
        SlashOutcome::SetDevHud(setting) => {
            let next = setting.unwrap_or(!slash_writers.hud_verbosity.dev_hud);
            slash_writers.hud_verbosity.dev_hud = next;
            push_system_chat_line(
                scene_state,
                format!("/devhud: {}", if next { "on" } else { "off" }),
            );
        }
        SlashOutcome::SetNetStatus(setting) => {
            let next = setting.unwrap_or(!slash_writers.net_status_visible.0);
            slash_writers.net_status_visible.0 = next;
            push_system_chat_line(
                scene_state,
                format!("/netstat: {}", if next { "on" } else { "off" }),
            );
        }
        SlashOutcome::SetRenderScale(setting) => {
            let g = &mut *slash_writers.graphics;
            if let Some(v) = setting {
                g.render_scale = v.clamp(0.25, 2.0);
                g.preset = ffxi_viewer_core::QualityPreset::Custom;
            }
            push_system_chat_line(
                scene_state,
                format!(
                    "/renderscale: {:.0}%{}",
                    g.render_scale() * 100.0,
                    if g.wants_render_scale() {
                        ""
                    } else {
                        " (native)"
                    }
                ),
            );
        }
        SlashOutcome::SetSky(op) => {
            use super::slash_commands::SkyOp;
            use ffxi_viewer_core::SkyStyle;

            let g = &mut *slash_writers.graphics;
            let next = match op {
                SkyOp::Status => g.sky_style(),
                SkyOp::Set(style) => style,
                SkyOp::Toggle => match g.sky_style() {
                    SkyStyle::Enhanced => SkyStyle::Vanilla,
                    SkyStyle::Vanilla => SkyStyle::Enhanced,
                },
            };
            g.sky_style = next;
            push_system_chat_line(scene_state, format!("/sky: {}", next.label()));
        }
        SlashOutcome::SetZoneLines(op) => {
            use super::slash_commands::ZoneLineOp;
            use ffxi_viewer_core::ZoneLineDisplay;

            let g = &mut *slash_writers.graphics;
            let next = match op {
                ZoneLineOp::Status => g.zone_line_display,
                ZoneLineOp::Set(mode) => mode,
                ZoneLineOp::Toggle => match g.zone_line_display {
                    ZoneLineDisplay::Off => ZoneLineDisplay::Pillar,
                    ZoneLineDisplay::Pillar => ZoneLineDisplay::Gate,
                    ZoneLineDisplay::Gate => ZoneLineDisplay::Off,
                },
            };
            g.zone_line_display = next;
            push_system_chat_line(scene_state, format!("/zoneline: {}", next.label()));
        }
        SlashOutcome::SetLights(op) => {
            use super::slash_commands::LightsOp;
            use ffxi_viewer_core::graphics_settings::DynamicLights;

            let g = &mut *slash_writers.graphics;
            let chat =
                match op {
                    LightsOp::Status => format!(
                    "/lights: {} · threshold {:.2} · intensity {:.0} · range {:.1} · flicker {}",
                    if g.dynamic_lights.enabled() { "on" } else { "off" },
                    g.light_threshold,
                    g.light_intensity,
                    g.light_range,
                    if g.light_flicker { "on" } else { "off" },
                ),
                    LightsOp::Enable(v) => {
                        let on = v.unwrap_or(!g.dynamic_lights.enabled());
                        g.dynamic_lights = if !on {
                            DynamicLights::Off
                        } else if g.dynamic_lights == DynamicLights::Off {
                            DynamicLights::Many
                        } else {
                            g.dynamic_lights
                        };
                        format!("/lights: {}", if on { "on" } else { "off" })
                    }
                    LightsOp::Threshold(v) => {
                        g.light_threshold = v;
                        format!("/lights threshold: {v:.2} (re-enter zone to re-detect)")
                    }
                    LightsOp::Intensity(v) => {
                        g.light_intensity = v;
                        format!("/lights intensity: {v:.0}")
                    }
                    LightsOp::Range(v) => {
                        g.light_range = v;
                        format!("/lights range: {v:.1}")
                    }
                    LightsOp::Flicker(v) => {
                        let f = v.unwrap_or(!g.light_flicker);
                        g.light_flicker = f;
                        format!("/lights flicker: {}", if f { "on" } else { "off" })
                    }
                };
            push_system_chat_line(scene_state, chat);
        }
        SlashOutcome::SetMinimap(op) => {
            use super::slash_commands::MinimapOp;
            use ffxi_viewer_core::minimap::MinimapMode;
            let chat = match op {
                MinimapOp::Status => {
                    let zone = scene_state.snapshot.zone_id;
                    let resolved = slash_writers
                        .minimap_state
                        .resolved_mode(*slash_writers.minimap_mode);
                    let top_down = if slash_writers.minimap_state.aabb.is_some() {
                        "baked"
                    } else {
                        "not baked"
                    };
                    format!(
                        "/minimap: mode={:?}→{:?} visible={} cull={:.1} zone={} | retail: {} | top-down: {}",
                        *slash_writers.minimap_mode,
                        resolved,
                        slash_writers.minimap_visible.0,
                        slash_writers.topdown_cull.top_cull_yalms,
                        zone.map(|z| z.to_string()).unwrap_or_else(|| "—".into()),
                        minimap_retail_desc(&slash_writers.minimap_state, zone),
                        top_down,
                    )
                }
                MinimapOp::Show => {
                    slash_writers.minimap_visible.0 = true;
                    "/minimap: shown".into()
                }
                MinimapOp::Hide => {
                    slash_writers.minimap_visible.0 = false;
                    "/minimap: hidden".into()
                }
                MinimapOp::Toggle => {
                    let next = !slash_writers.minimap_visible.0;
                    slash_writers.minimap_visible.0 = next;
                    format!("/minimap: {}", if next { "shown" } else { "hidden" })
                }
                MinimapOp::ModeTopDown => {
                    *slash_writers.minimap_mode = MinimapMode::TopDown;
                    "/minimap: mode=top-down".into()
                }
                MinimapOp::ModeRetail => {
                    *slash_writers.minimap_mode = MinimapMode::Retail;
                    let zone = scene_state.snapshot.zone_id;
                    format!(
                        "/minimap: mode=retail | {}",
                        minimap_retail_desc(&slash_writers.minimap_state, zone)
                    )
                }
                MinimapOp::ModeAuto => {
                    *slash_writers.minimap_mode = MinimapMode::Auto;
                    "/minimap: mode=auto".into()
                }
                MinimapOp::SetCull(v) => {
                    slash_writers.topdown_cull.top_cull_yalms = v;
                    format!("/minimap: cull={v:.1} yalms (re-baking next frame)")
                }
                MinimapOp::ZoomIn => {
                    let half = ffxi_viewer_core::minimap::zone_half_span(
                        slash_writers
                            .minimap_state
                            .active_aabb(*slash_writers.minimap_mode),
                    );
                    slash_writers
                        .minimap_zoom
                        .zoom_by(1.0 / ffxi_viewer_core::minimap::ZOOM_STEP_FACTOR, half);
                    slash_writers.minimap_view.idle_frames = 0;
                    format_zoom_status(&slash_writers.minimap_zoom)
                }
                MinimapOp::ZoomOut => {
                    let half = ffxi_viewer_core::minimap::zone_half_span(
                        slash_writers
                            .minimap_state
                            .active_aabb(*slash_writers.minimap_mode),
                    );
                    slash_writers
                        .minimap_zoom
                        .zoom_by(ffxi_viewer_core::minimap::ZOOM_STEP_FACTOR, half);
                    slash_writers.minimap_view.idle_frames = 0;
                    format_zoom_status(&slash_writers.minimap_zoom)
                }
                MinimapOp::ZoomFit => {
                    slash_writers.minimap_zoom.radius_yalms = None;
                    slash_writers.minimap_view.idle_frames = 0;
                    "/minimap zoom: fit-to-zone".into()
                }
                MinimapOp::ZoomSet(r) => {
                    let clamped = r.max(ffxi_viewer_core::minimap::ZOOM_MIN_RADIUS);
                    slash_writers.minimap_zoom.radius_yalms = Some(clamped);
                    slash_writers.minimap_view.idle_frames = 0;
                    format!("/minimap zoom: radius={clamped:.0} yalms")
                }
                MinimapOp::ZoomReset => {
                    *slash_writers.minimap_zoom = ffxi_viewer_core::minimap::MinimapZoom::default();
                    slash_writers.minimap_view.pan_offset_xz = bevy::math::Vec2::ZERO;
                    slash_writers.minimap_view.idle_frames = 0;
                    "/minimap zoom: reset to defaults".into()
                }
            };
            push_system_chat_line(scene_state, chat);
        }
        SlashOutcome::SetSound(op) => {
            use super::slash_commands::SoundOp;
            let mute = &mut *slash_writers.audio_mute;

            let apply = |cur: &mut bool, target: Option<bool>| {
                *cur = target.unwrap_or(!*cur);
            };
            let chat = match op {
                SoundOp::Status => format!(
                    "/sound: bgm={} sfx={}",
                    if mute.bgm { "off" } else { "on" },
                    if mute.sfx { "off" } else { "on" },
                ),
                SoundOp::SetBoth(target) => {
                    apply(&mut mute.bgm, target);
                    apply(&mut mute.sfx, target);
                    format!(
                        "/sound: bgm={} sfx={}",
                        if mute.bgm { "off" } else { "on" },
                        if mute.sfx { "off" } else { "on" },
                    )
                }
                SoundOp::SetBgm(target) => {
                    apply(&mut mute.bgm, target);
                    format!("/sound bgm: {}", if mute.bgm { "off" } else { "on" })
                }
                SoundOp::SetSfx(target) => {
                    apply(&mut mute.sfx, target);
                    format!("/sound sfx: {}", if mute.sfx { "off" } else { "on" })
                }
            };
            push_system_chat_line(scene_state, chat);
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
        SlashOutcome::ShopBuyRow { shop_index, qty } => match scene_state.snapshot.shop.as_ref() {
            Some(shop) => {
                let _ = cmd_tx.try_send(AgentCommand::ShopBuy {
                    shop_no: shop.offset_index,
                    shop_index,
                    qty,
                });
            }
            None => push_system_chat_line(scene_state, "/buy: no shop is open".into()),
        },
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
        SlashOutcome::OpenMenu(kind) => {
            let label: std::borrow::Cow<'static, str> = match kind {
                ffxi_viewer_core::MenuKind::Magic => "Magic".into(),
                ffxi_viewer_core::MenuKind::Abilities => "Abilities".into(),
                ffxi_viewer_core::MenuKind::Items => "Items".into(),
                ffxi_viewer_core::MenuKind::Equipment => "Equipment".into(),
                ffxi_viewer_core::MenuKind::Root => "Root".into(),
                ffxi_viewer_core::MenuKind::Config => "Config".into(),
                ffxi_viewer_core::MenuKind::Graphics => "Graphics".into(),
                ffxi_viewer_core::MenuKind::Status => "Status".into(),

                ffxi_viewer_core::MenuKind::EquipSlot(slot) => format!("EquipSlot({slot})").into(),
            };
            push_system_chat_line(scene_state, format!("[menu] opened {label}"));
        }
    }
}

fn apply_copy_toasts(n: usize, scene_state: &mut SceneState) {
    let toasts = &scene_state.local_toasts;
    if toasts.is_empty() {
        push_system_chat_line(scene_state, "/copy: no toasts to copy".into());
        return;
    }
    let take = n.min(toasts.len());
    let start = toasts.len() - take;

    let payload: String = toasts[start..]
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    match arboard::Clipboard::new() {
        Ok(mut cb) => match cb.set_text(payload) {
            Ok(()) => {
                push_system_chat_line(scene_state, format!("/copy: {take} toast(s) on clipboard"));
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

        _ => "Mod+",
    }
}

fn push_system_chat_line(scene_state: &mut SceneState, text: String) {
    scene_state.push_local_toast(system_chat_line(text));
}

fn format_zoom_status(zoom: &ffxi_viewer_core::minimap::MinimapZoom) -> String {
    match zoom.radius_yalms {
        Some(r) => format!("/minimap zoom: radius={r:.0} yalms"),
        None => "/minimap zoom: fit-to-zone".into(),
    }
}

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

fn mirror_heal_stance(cmd: &AgentCommand, rest: &mut ffxi_viewer_core::combat_stance::RestStance) {
    use ffxi_viewer_core::combat_stance::RestKind;
    let AgentCommand::Heal { mode } = cmd else {
        return;
    };
    let next = match mode {
        crate::state::HealMode::On => RestKind::Heal,
        crate::state::HealMode::Off => match rest.kind {
            RestKind::Heal => RestKind::None,
            other => other,
        },
        crate::state::HealMode::Toggle => match rest.kind {
            RestKind::Heal => RestKind::None,
            _ => RestKind::Heal,
        },
    };
    rest.kind = next;
}

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

#[derive(Debug, Clone, PartialEq)]
enum MenuDispatch {
    CommandWithToast { cmd: AgentCommand, toast: String },

    OpenSubmenu(MenuKind),

    KeybindUpdate(KeybindUpdate),

    NotImplemented(String),
}

fn apply_graphics_cycle(
    cursor: usize,
    delta: i32,
    graphics: &mut ffxi_viewer_core::GraphicsSettings,
) {
    use ffxi_viewer_core::graphics_settings::GRAPHICS_FIELDS;
    if let Some(&field) = GRAPHICS_FIELDS.get(cursor) {
        graphics.cycle(field, delta);
    }
}

fn resolve_menu_entry(kind: MenuKind, label: &str) -> MenuDispatch {
    match (kind, label) {
        (MenuKind::Root, "Logout") => MenuDispatch::CommandWithToast {
            cmd: AgentCommand::ReqLogout {
                kind: ReqLogoutKind::LogoutToggle,
            },
            toast: "[menu] Logout requested (~30s; immediate for GMs / \
                    in Mog House). Toggle again or `/logout off` to cancel."
                .into(),
        },

        (MenuKind::Root, "Config") => MenuDispatch::OpenSubmenu(MenuKind::Config),

        (MenuKind::Root, "Graphics") => MenuDispatch::OpenSubmenu(MenuKind::Graphics),

        (MenuKind::Root, "Magic") => MenuDispatch::OpenSubmenu(MenuKind::Magic),
        (MenuKind::Root, "Abilities") => MenuDispatch::OpenSubmenu(MenuKind::Abilities),
        (MenuKind::Root, "Items") => MenuDispatch::OpenSubmenu(MenuKind::Items),
        (MenuKind::Root, "Key Items") => {
            MenuDispatch::NotImplemented("Key Items — pending submenu (s2c 0x055 decoded)".into())
        }
        (MenuKind::Root, "Equipment") => MenuDispatch::OpenSubmenu(MenuKind::Equipment),

        (MenuKind::Root, "Status") => MenuDispatch::OpenSubmenu(MenuKind::Status),

        (MenuKind::Magic, _) => {
            MenuDispatch::NotImplemented("Magic — pending Stage 2 (learned-spell decoder)".into())
        }
        (MenuKind::Abilities, _) => MenuDispatch::NotImplemented(
            "Abilities — pending Stage 2 (s2c 0x119 abil_recast)".into(),
        ),
        (MenuKind::Items, _) => {
            MenuDispatch::NotImplemented("Items — pending Stage 3 (inventory submenu)".into())
        }
        (MenuKind::Equipment, _) => MenuDispatch::NotImplemented(
            "Equipment — pending Stage 1 (s2c 0x050 equip_list)".into(),
        ),

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

fn confirm_menu_at_cursor(
    bindings: &mut Bindings,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    keybinds_state: &mut KeybindsStateRes,
    graphics: &mut ffxi_viewer_core::GraphicsSettings,
    status_profile_open: &mut ffxi_viewer_core::hud::status_panel::StatusProfileOpen,
    dynamic: &ffxi_viewer_core::hud::menu::DynamicMenu,
    target_id: Option<u32>,
    self_pos: ffxi_viewer_wire::Vec3,
) -> Option<InputMode> {
    let (kind, cursor) = {
        let level = stack.current()?;
        (level.kind, level.cursor)
    };

    if matches!(kind, MenuKind::Status) {
        use ffxi_viewer_core::hud::status_panel::{StatusEntryKind, STATUS_ENTRIES};
        let entry = STATUS_ENTRIES.get(cursor)?;
        match entry.kind {
            StatusEntryKind::Profile => {
                status_profile_open.0 = true;
            }
            StatusEntryKind::PlayTime => {
                let line =
                    ffxi_viewer_core::hud::status_panel::play_time_chat_line(&scene_state.snapshot);
                push_system_chat_line(scene_state, line);
            }

            StatusEntryKind::MasterLevels | StatusEntryKind::MeritPoints => {
                push_system_chat_line(
                    scene_state,
                    format!("[menu] {} — not available", entry.label),
                );
            }

            StatusEntryKind::JobLevels
            | StatusEntryKind::CombatSkill
            | StatusEntryKind::MagicSkill
            | StatusEntryKind::CraftSkill
            | StatusEntryKind::Currencies
            | StatusEntryKind::Currencies2
            | StatusEntryKind::Unity
            | StatusEntryKind::JobPoints => {
                push_system_chat_line(
                    scene_state,
                    format!("[menu] {} — not yet decoded", entry.label),
                );
            }
        }
        return None;
    }
    if matches!(kind, MenuKind::Graphics) {
        if cursor == ffxi_viewer_core::hud::menu::GRAPHICS_RESET_SLOT {
            graphics.reset_to_default();
            push_system_chat_line(scene_state, "[menu] Graphics reset to High".into());
        } else {
            apply_graphics_cycle(cursor, 1, graphics);
        }
        return None;
    }

    if matches!(kind, MenuKind::Equipment) {
        let slot = (cursor as u8).min(15);
        stack.push(MenuKind::EquipSlot(slot));
        return None;
    }

    if ffxi_viewer_core::hud::menu::is_dynamic(kind) {
        if let Some(action) = ffxi_viewer_core::hud::menu::entry_action(kind, cursor, dynamic) {
            let entities = scene_state.snapshot.entities.clone();
            dispatch_dynamic_menu_action(
                action,
                target_id,
                self_pos,
                &entities,
                cmd_tx,
                scene_state,
            );
            return Some(InputMode::World);
        }

        push_system_chat_line(scene_state, format!("[menu] {kind:?} list is empty"));
        return None;
    }
    let label = ffxi_viewer_core::hud::menu::entry_label(kind, cursor, dynamic);
    match resolve_menu_entry(kind, label) {
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
    }
}

fn dispatch_dynamic_menu_action(
    action: ffxi_viewer_core::hud::menu::DynamicMenuAction,
    target_id: Option<u32>,
    self_pos: ffxi_viewer_wire::Vec3,
    entities: &[ffxi_viewer_wire::Entity],
    cmd_tx: &Sender<AgentCommand>,
    scene_state: &mut SceneState,
) {
    use ffxi_viewer_core::hud::menu::DynamicMenuAction as A;
    let self_char_id = scene_state.snapshot.self_char_id;
    let pick_target = |require: bool| -> Option<(u32, u16)> {
        if let Some(id) = target_id {
            if let Some(ent) = entities.iter().find(|e| e.id == id) {
                return Some((ent.id, ent.act_index));
            }
        }
        if require {
            return None;
        }

        let me_id = self_char_id?;
        let me = entities.iter().find(|e| e.id == me_id)?;
        Some((me.id, me.act_index))
    };
    let self_target = || -> Option<(u32, u16)> {
        let me_id = self_char_id?;
        let me = entities.iter().find(|e| e.id == me_id)?;
        Some((me.id, me.act_index))
    };

    let (kind_name, cmd) = match action {
        A::CastSpell { spell_id } => {
            let self_only =
                ffxi_proto::valid_target::spell(spell_id).is_some_and(|f| f.is_self_only());
            let resolved = if self_only {
                self_target()
            } else {
                pick_target(false)
            };
            let Some((tid, tidx)) = resolved else {
                push_system_chat_line(
                    scene_state,
                    "[menu] cast: no target and self not resolved yet".into(),
                );
                return;
            };
            (
                "cast",
                AgentCommand::Action {
                    target_id: tid,
                    target_index: tidx,
                    kind: ActionKind::CastMagic {
                        spell_id: spell_id as u32,
                        pos_x: self_pos.x,
                        pos_y: self_pos.y,
                        pos_z: self_pos.z,
                    },
                },
            )
        }
        A::JobAbility { ability_id } | A::PetAbility { ability_id } => {
            let self_only =
                ffxi_proto::valid_target::ability(ability_id).is_some_and(|f| f.is_self_only());
            let resolved = if self_only {
                self_target()
            } else {
                pick_target(false)
            };
            let Some((tid, tidx)) = resolved else {
                push_system_chat_line(scene_state, "[menu] ability: no target".into());
                return;
            };
            (
                "ability",
                AgentCommand::Action {
                    target_id: tid,
                    target_index: tidx,
                    kind: ActionKind::JobAbility {
                        ability_id: ability_id as u32,
                    },
                },
            )
        }
        A::Weaponskill { skill_id } => {
            let Some((tid, tidx)) = pick_target(true) else {
                push_system_chat_line(
                    scene_state,
                    "[menu] weaponskill: requires a battle target".into(),
                );
                return;
            };
            (
                "weaponskill",
                AgentCommand::Action {
                    target_id: tid,
                    target_index: tidx,
                    kind: ActionKind::Weaponskill {
                        skill_id: skill_id as u32,
                    },
                },
            )
        }
        A::UseItem {
            container,
            index,
            item_no,
        } => {
            let (tid, tidx) = pick_target(false).unwrap_or((0, 0));
            (
                "useitem",
                AgentCommand::UseItem {
                    container,
                    slot: index,
                    item_no: item_no as u32,
                    target_id: tid,
                    target_index: tidx,
                },
            )
        }
        A::EquipItem {
            container,
            container_index,
            equip_slot,
        } => (
            "equip",
            AgentCommand::Equip {
                container,
                container_index,
                equip_slot,
            },
        ),
    };
    if let Err(e) = cmd_tx.try_send(cmd) {
        push_system_chat_line(scene_state, format!("[menu] {kind_name} dropped: {e}"));
    }
}

fn confirm_dialog_choice(choice: u32, scene_state: &mut SceneState, cmd_tx: &Sender<AgentCommand>) {
    if let Some(d) = scene_state.snapshot.dialog.as_ref() {
        // EVENT_END validates against the event id, which the trigger carries in
        // EventPara (event_num is the zone) — see event_trigger_ids in session.rs.
        let _ = cmd_tx.try_send(AgentCommand::EndEventChoice {
            event_id: d.npc_id,
            act_index: d.act_index,
            event_num: d.event_para,
            choice,
        });
    }
}

fn confirm_quick_action_at_cursor(
    state: &QuickActionState,
    scene_state: &mut SceneState,
    target_id: Option<u32>,
    entities: &[ffxi_viewer_wire::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let label = ffxi_viewer_core::hud::quick_action::entry_label(state.has_target, state.cursor);
    let target_ent = target_id.and_then(|id| entities.iter().find(|e| e.id == id));
    match resolve_quick_action(label, target_ent) {
        QuickActionDispatch::Command(cmd) => {
            if let Err(e) = cmd_tx.try_send(cmd) {
                push_system_chat_line(scene_state, format!("[quick] dispatch dropped: {e}"));
            }
            Some(InputMode::World)
        }
        QuickActionDispatch::SystemMessage(msg) => {
            push_system_chat_line(scene_state, msg);
            Some(InputMode::World)
        }
        QuickActionDispatch::NotImplemented(label) => {
            push_system_chat_line(scene_state, format!("[quick] {label} — not implemented"));
            Some(InputMode::World)
        }
        QuickActionDispatch::OpenMenu(kind) => {
            let mut stack = MenuStack::root();
            stack.push(kind);
            Some(InputMode::Menu(stack))
        }
    }
}

pub fn mouse_nav_dispatch_system(
    mut menu_events: MessageReader<ffxi_viewer_core::hud::menu::MenuRowActivated>,
    mut dialog_events: MessageReader<ffxi_viewer_core::hud::dialog::DialogChoiceActivated>,
    mut qa_events: MessageReader<ffxi_viewer_core::hud::quick_action::QuickActionActivated>,
    mut ta_events: MessageReader<ffxi_viewer_core::hud::target_action_menu::TargetActionActivated>,
    cmd_tx: Res<CommandTx>,
    mut bindings: ResMut<Bindings>,
    mut keybinds_state: ResMut<KeybindsStateRes>,
    mut mode: ResMut<InputMode>,
    target: Res<Target>,
    mut scene_state: ResMut<SceneState>,
    mut graphics: ResMut<ffxi_viewer_core::GraphicsSettings>,
    mut status_profile_open: ResMut<ffxi_viewer_core::hud::status_panel::StatusProfileOpen>,
    dynamic_menu: Res<ffxi_viewer_core::hud::menu::DynamicMenu>,
    mut check_target: ResMut<ffxi_viewer_core::hud::check_view::CheckTarget>,
    mut trade_state: ResMut<ffxi_viewer_core::hud::trade::TradeState>,
    mut select_target: ResMut<SelectTargetMode>,
) {
    let entities = scene_state.snapshot.entities.clone();
    let current_target = target.id;
    let self_pos = scene_state.snapshot.self_pos.pos;

    for ev in menu_events.read() {
        if let InputMode::Menu(stack) = &mut *mode {
            if let Some(level) = stack.current_mut() {
                level.cursor = ev.slot;
            }
            if let Some(next) = confirm_menu_at_cursor(
                &mut bindings,
                stack,
                &mut scene_state,
                &cmd_tx.0,
                &mut keybinds_state,
                &mut graphics,
                &mut status_profile_open,
                &dynamic_menu,
                current_target,
                self_pos,
            ) {
                *mode = next;
            }
        }
    }

    for ev in dialog_events.read() {
        if let InputMode::Dialog(cursor) = &mut *mode {
            cursor.cursor = ev.choice;
            confirm_dialog_choice(ev.choice, &mut scene_state, &cmd_tx.0);
        }
    }

    for ev in qa_events.read() {
        if let InputMode::QuickAction(state) = &mut *mode {
            state.cursor = ev.slot;
            let snapshot = QuickActionState {
                cursor: state.cursor,
                has_target: state.has_target,
            };
            if let Some(next) = confirm_quick_action_at_cursor(
                &snapshot,
                &mut scene_state,
                current_target,
                &entities,
                &cmd_tx.0,
            ) {
                *mode = next;
            }
        }
    }

    for ev in ta_events.read() {
        if let InputMode::TargetAction(state) = &mut *mode {
            state.cursor = ev.slot;

            let entries = ffxi_viewer_core::hud::overlay::RETAIL.resolve_target_actions(&state.ctx);
            if let Some(next) = confirm_target_action_at_cursor(
                state,
                &entries,
                &mut scene_state,
                current_target,
                &entities,
                &cmd_tx.0,
                &mut check_target,
                &mut trade_state,
                &mut select_target,
            ) {
                *mode = next;
            }
        }
    }
}

fn handle_menu_key(
    key: &Key,
    bindings: &mut Bindings,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    keybinds_state: &mut KeybindsStateRes,
    graphics: &mut ffxi_viewer_core::GraphicsSettings,
    status_profile_open: &mut ffxi_viewer_core::hud::status_panel::StatusProfileOpen,
    dynamic: &ffxi_viewer_core::hud::menu::DynamicMenu,
    target_id: Option<u32>,
    self_pos: ffxi_viewer_wire::Vec3,
) -> Option<InputMode> {
    let (kind, cursor) = {
        let level = stack.current()?;
        (level.kind, level.cursor)
    };
    let entry_count = ffxi_viewer_core::hud::menu::entry_count(kind, dynamic);

    if matches!(kind, MenuKind::Graphics) {
        if bindings.matches_logical(Action::NavLeft, key) {
            apply_graphics_cycle(cursor, -1, graphics);
            return None;
        }
        if bindings.matches_logical(Action::NavRight, key) {
            apply_graphics_cycle(cursor, 1, graphics);
            return None;
        }
    }

    if bindings.matches_logical(Action::NavUp, key) {
        let level = stack.current_mut()?;
        level.cursor = if cursor == 0 {
            entry_count.saturating_sub(1)
        } else {
            cursor - 1
        };
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        let level = stack.current_mut()?;
        let next = cursor + 1;
        level.cursor = if next >= entry_count { 0 } else { next };
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        return confirm_menu_at_cursor(
            bindings,
            stack,
            scene_state,
            cmd_tx,
            keybinds_state,
            graphics,
            status_profile_open,
            dynamic,
            target_id,
            self_pos,
        );
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        if matches!(kind, MenuKind::Status) {
            status_profile_open.0 = false;
        }
        return if !stack.pop() {
            Some(InputMode::World)
        } else {
            None
        };
    }
    None
}

fn handle_dialog_key(
    key: &Key,
    bindings: &Bindings,
    cursor: &mut DialogCursor,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    // Plain speech (no choices) clamps to 0 and still confirms/advances on Enter.
    let max_index = scene_state
        .snapshot
        .dialog
        .as_ref()
        .map(|d| d.choices.len() as u32)
        .unwrap_or(0)
        .min(ffxi_viewer_core::hud::dialog::MAX_OPTION_ROWS)
        .saturating_sub(1);
    if bindings.matches_logical(Action::NavUp, key) {
        if cursor.cursor > 0 {
            cursor.cursor -= 1;
        }
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        if cursor.cursor < max_index {
            cursor.cursor += 1;
        }
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        confirm_dialog_choice(cursor.cursor.min(max_index), scene_state, cmd_tx);
        return None;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        // Reconcile via the session snapshot; clearing here flickers multi-frame events.
        let _ = cmd_tx.try_send(AgentCommand::EndEvent);
        return None;
    }
    None
}

#[derive(Debug, Clone, PartialEq)]
enum QuickActionDispatch {
    Command(AgentCommand),
    SystemMessage(String),
    NotImplemented(String),

    OpenMenu(MenuKind),
}

fn resolve_quick_action(
    label: &str,
    target: Option<&ffxi_viewer_wire::Entity>,
) -> QuickActionDispatch {
    match label {
        "Check" => match target {
            Some(ent) => QuickActionDispatch::Command(AgentCommand::CheckTarget {
                target_id: ent.id,
                target_index: ent.act_index,
                kind: CheckKind::Check,
            }),
            None => QuickActionDispatch::SystemMessage("[quick] Check: no target".into()),
        },

        "Attack" => match target {
            Some(ent) => QuickActionDispatch::Command(AgentCommand::Action {
                target_id: ent.id,
                target_index: ent.act_index,
                kind: ActionKind::Attack,
            }),
            None => QuickActionDispatch::SystemMessage("[quick] Attack: no target".into()),
        },

        "Talk" => match target {
            Some(ent) => QuickActionDispatch::Command(AgentCommand::Action {
                target_id: ent.id,
                target_index: ent.act_index,
                kind: ActionKind::Talk,
            }),
            None => QuickActionDispatch::SystemMessage("[quick] Talk: no target".into()),
        },

        "Magic" => QuickActionDispatch::OpenMenu(MenuKind::Magic),
        "Abilities" => QuickActionDispatch::OpenMenu(MenuKind::Abilities),
        "Items" => QuickActionDispatch::OpenMenu(MenuKind::Items),

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
        state.cursor = if state.cursor == 0 {
            entry_count.saturating_sub(1)
        } else {
            state.cursor - 1
        };
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        let next = state.cursor + 1;
        state.cursor = if next >= entry_count { 0 } else { next };
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        return confirm_quick_action_at_cursor(state, scene_state, target_id, entities, cmd_tx);
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        return Some(InputMode::World);
    }
    None
}

fn handle_passive_cursor_key(
    key: &Key,
    bindings: &Bindings,
    chat_scroll: &mut ChatScroll,
    scene_state: &SceneState,
) -> Option<InputMode> {
    let max_back = ffxi_viewer_core::snapshot::rendered_chat(scene_state).len();

    if bindings.matches_logical(Action::NavUp, key) {
        if chat_scroll.rows + 1 < max_back {
            chat_scroll.rows += 1;
        }
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        chat_scroll.rows = chat_scroll.rows.saturating_sub(1);
        return None;
    }
    if bindings.matches_logical(Action::PageUp, key) {
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

    None
}

#[cfg(test)]
mod reqlogout_ack_tests {
    use super::*;

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
            animation: 0,
            animationsub: 0,
            status: 0,
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
        let result = resolve_quick_action("Macros", Some(&ent));
        assert_eq!(result, QuickActionDispatch::NotImplemented("Macros".into()),);
    }

    #[test]
    fn contextual_action_categories_open_their_menu() {
        for (label, expected) in [
            ("Magic", MenuKind::Magic),
            ("Abilities", MenuKind::Abilities),
            ("Items", MenuKind::Items),
        ] {
            let result = resolve_quick_action(label, None);
            assert_eq!(
                result,
                QuickActionDispatch::OpenMenu(expected),
                "{label} should open {expected:?}",
            );
        }
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
        for label in ["Party", "Search", "Macros"] {
            assert_eq!(
                resolve_menu_entry(MenuKind::Root, label),
                MenuDispatch::NotImplemented(label.into()),
                "{label} should still be a stub"
            );
        }
    }
}
