use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::{PresentMode, PrimaryWindow};
use kuluu_render::dat_mmb::LoadMmbRequest;
use kuluu_render::dat_mzb::LoadMzbRequest;
use kuluu_render::hud::chat_panel::{ActiveChatTab, ChatScroll};
use kuluu_render::{
    Action, Bindings, ChatBuffer, ChatHistory, DialogCursor, InputMode, MenuKind, MenuStack,
    Preset, QuickActionState, SceneState, Target,
};

use super::debug_heights::DebugHeightsRequest;

mod check;
pub use check::bazaar_mode_sync_system;
use check::{handle_bazaar_key, handle_check_key};

mod auction;
pub use auction::auction_mode_sync_system;
use auction::{auction_click, handle_auction_key};

mod delivery;
pub use delivery::delivery_mode_sync_system;
use delivery::handle_delivery_key;

mod map_screen;

mod menu;
use menu::{confirm_menu_at_cursor, handle_menu_key};

mod slash_apply;
use slash_apply::apply_slash_outcome;

mod target_action;
use target_action::{confirm_target_action_at_cursor, handle_target_action_key, handle_world_key};

#[derive(Resource, Default)]
pub struct CaptureMode {
    pub active: bool,

    pub restore_limiter: Option<bevy_framepace::Limiter>,
}

#[derive(SystemParam)]
pub struct SlashWriters<'w, 's> {
    pub load_mmb: MessageWriter<'w, LoadMmbRequest>,
    pub load_mzb: MessageWriter<'w, LoadMzbRequest>,
    pub set_sub_area: MessageWriter<'w, kuluu_render::sub_area_activation::SetSubArea>,
    pub debug_heights: MessageWriter<'w, DebugHeightsRequest>,

    pub logout_requested: MessageWriter<'w, kuluu_render::hud::logout_countdown::LogoutRequested>,

    pub framepace: ResMut<'w, bevy_framepace::FramepaceSettings>,

    pub primary_window: Query<'w, 's, &'static mut Window, With<PrimaryWindow>>,

    pub capture_mode: ResMut<'w, CaptureMode>,

    pub event_log: ResMut<'w, kuluu_render::EventLog>,

    pub sfx_event: MessageWriter<'w, kuluu_render::audio::SfxEvent>,

    pub screenshot: MessageWriter<'w, super::screenshot::ScreenshotRequest>,

    pub graphics: ResMut<'w, kuluu_render::GraphicsSettings>,

    pub hud_verbosity: ResMut<'w, kuluu_render::hud::HudVerbosity>,

    pub hud_panels: ResMut<'w, kuluu_render::hud::HudPanels>,

    pub net_status_visible: ResMut<'w, kuluu_render::hud::network_status::NetStatusVisible>,

    pub vana_clock: Res<'w, kuluu_render::vana_time::VanaClock>,

    pub vana_clock_visible: ResMut<'w, kuluu_render::hud::vana_clock::VanaClockVisible>,

    pub minimap_mode: ResMut<'w, kuluu_render::minimap::MinimapMode>,

    pub minimap_visible: ResMut<'w, kuluu_render::minimap::MinimapVisible>,

    pub topdown_cull: ResMut<'w, kuluu_render::minimap::topdown::TopdownCullPolicy>,

    pub audio_mute: ResMut<'w, kuluu_render::audio::AudioMuteState>,

    pub minimap_zoom: ResMut<'w, kuluu_render::minimap::MinimapZoom>,

    pub minimap_view: ResMut<'w, kuluu_render::minimap::MinimapView>,

    pub minimap_state: Res<'w, kuluu_render::minimap::MinimapState>,

    pub rest_stance: ResMut<'w, kuluu_render::combat_stance::RestStance>,

    pub status_profile_open: ResMut<'w, kuluu_render::hud::status_panel::StatusProfileOpen>,

    pub sort_options: ResMut<'w, kuluu_render::hud::item_detail::SortOptions>,

    pub item_menu_focus: ResMut<'w, kuluu_render::hud::item_detail::ItemMenuFocus>,

    pub item_screen_container: ResMut<'w, kuluu_render::hud::item_screen::ItemScreenContainer>,

    pub check_target: ResMut<'w, kuluu_render::hud::check_view::CheckTarget>,

    pub bazaar_state: ResMut<'w, kuluu_render::hud::bazaar_view::BazaarScreenState>,

    pub trade_state: ResMut<'w, kuluu_render::hud::trade::TradeState>,

    pub trade_intent: MessageWriter<'w, kuluu_render::hud::trade::TradeIntent>,

    pub delivery_state: ResMut<'w, kuluu_render::hud::delivery::DeliveryScreenState>,

    pub delivery_inv: Res<'w, kuluu_render::hud::delivery::DeliveryInventory>,

    pub auction_state: ResMut<'w, kuluu_render::hud::auction::AuctionScreenState>,

    pub auction_inv: Res<'w, kuluu_render::hud::auction::AuctionSellInventory>,

    pub select_target: ResMut<'w, SelectTargetMode>,

    pub fishing_spot: Res<'w, kuluu_render::fishing_spot::FishingSpot>,

    pub active_chat_tab: ResMut<'w, ActiveChatTab>,

    pub chat_history: ResMut<'w, ChatHistory>,

    pub map_screen_state: ResMut<'w, kuluu_render::hud::map_screen::MapScreenState>,

    pub map_markers: ResMut<'w, kuluu_render::hud::map_screen::MapMarkers>,

    pub map_view: Res<'w, kuluu_render::hud::map_screen::MapView>,

    pub death_prompt: ResMut<'w, kuluu_render::hud::death_prompt::DeathPromptSelection>,

    pub(crate) dat_root: Res<'w, super::DatRootRes>,

    /// Absent when no config dir resolved, which makes `/overlay` read-only.
    pub overlay_store: Option<Res<'w, crate::overlay_store::OverlayStoreRes>>,
}

/// Real keyboard events plus the pad-synthesized ones
/// (`gamepad_input::PadKeyEvent`); the pad channel is separate so Bevy's
/// `keyboard_input_system` never mistakes synthetic presses for held keys.
#[derive(SystemParam)]
pub struct KeyEventStreams<'w, 's> {
    pub keyboard: MessageReader<'w, 's, KeyboardInput>,
    pub pad: MessageReader<'w, 's, super::gamepad_input::PadKeyEvent>,
}

#[derive(SystemParam)]
pub struct MenuConfirmWriters<'w> {
    pub graphics: ResMut<'w, kuluu_render::GraphicsSettings>,
    pub status_profile_open: ResMut<'w, kuluu_render::hud::status_panel::StatusProfileOpen>,
    pub hud_panels: ResMut<'w, kuluu_render::hud::HudPanels>,
    pub net_status: ResMut<'w, kuluu_render::hud::network_status::NetStatusVisible>,
    pub audio_mute: ResMut<'w, kuluu_render::audio::AudioMuteState>,
    pub vana_clock: Res<'w, kuluu_render::vana_time::VanaClock>,
    pub vana_clock_visible: ResMut<'w, kuluu_render::hud::vana_clock::VanaClockVisible>,
    pub item_screen_container: ResMut<'w, kuluu_render::hud::item_screen::ItemScreenContainer>,
}
use tokio::sync::mpsc::Sender;

use crate::keybinds_store::KeybindsStateRes;
use crate::view_native::input::{CommandTx, SelectTargetMode};
use crate::view_native::slash_commands::{
    parse_slash, system_chat_line, KeybindUpdate, SlashOutcome, SubAreaOp,
};
use kuluu_session::state::{ActionKind, AgentCommand, CheckKind, ReqLogoutKind};

pub(crate) fn text_input_system(
    mut events: KeyEventStreams,
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

    mut draw_distance: ResMut<kuluu_render::dat_mzb::DrawDistance>,

    mut chat_scroll: ResMut<ChatScroll>,

    dynamic_menu: Res<kuluu_render::hud::menu::DynamicMenu>,
) {
    let entities = scene_state.snapshot.entities.clone();
    let self_pos = scene_state.snapshot.self_pos.pos;
    let current_target = target.id;
    let engaged = matches!(
        scene_state.snapshot.current_goal,
        Some(kuluu_snapshot::ReactorGoal::Engaged { .. })
    );

    let target_changed = target.is_changed();

    let pad_synth: Vec<KeyboardInput> = events.pad.read().map(|e| e.0.clone()).collect();
    for ev in events.keyboard.read().chain(pad_synth.iter()) {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &mut *mode {
            InputMode::World => {
                if kuluu_render::hud::death_prompt::is_dead(&scene_state) {
                    let offer = scene_state.snapshot.death_menu_offer;
                    slash_writers.death_prompt.sync(offer);
                    if let Some(offer) = offer {
                        if bindings.matches_logical(Action::NavUp, &ev.logical_key)
                            || bindings.matches_logical(Action::NavDown, &ev.logical_key)
                        {
                            slash_writers.death_prompt.toggle();
                            continue;
                        }
                        let accept =
                            if bindings.matches_logical(Action::NavConfirm, &ev.logical_key) {
                                Some(slash_writers.death_prompt.accepts_offer())
                            } else if bindings.matches_logical(Action::NavCancel, &ev.logical_key) {
                                Some(false)
                            } else {
                                None
                            };
                        if let Some(accept) = accept {
                            if let Err(e) = cmd_tx
                                .0
                                .try_send(death_menu_response_command(offer, accept))
                            {
                                push_system_chat_line(
                                    &mut scene_state,
                                    format!("death-menu response dropped (channel issue): {e}"),
                                );
                            }
                            continue;
                        }
                    } else if bindings.matches_logical(Action::ConfirmAction, &ev.logical_key) {
                        if let Err(e) = cmd_tx.0.try_send(AgentCommand::ReturnToHomePoint) {
                            push_system_chat_line(
                                &mut scene_state,
                                format!("/return dropped (channel issue): {e}"),
                            );
                        }
                        continue;
                    }
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
                if bindings.matches_logical(Action::SelectActiveWindow, &ev.logical_key) {
                    *mode = InputMode::PassiveCursor(
                        kuluu_render::input_mode::PassiveCursorState::fresh_chat(),
                    );
                    continue;
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
                    kuluu_render::hud::menu::any_usable_item(&scene_state.snapshot),
                    slash_writers.fishing_spot.0.is_ready(),
                    &cmd_tx.0,
                ) {
                    *mode = next;
                }
            }
            InputMode::Chat(buffer) => {
                let action = handle_chat_key(
                    &ev.logical_key,
                    &bindings,
                    buffer,
                    &slash_writers.chat_history,
                );
                let fishing_gate = slash_writers.fishing_spot.0;
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
                    fishing_gate,
                    &mut slash_writers,
                    &mut draw_distance,
                );
            }
            InputMode::Menu(stack) => {
                if let Some(next) = handle_menu_key(
                    &ev.logical_key,
                    ev.key_code,
                    &mut bindings,
                    stack,
                    &mut scene_state,
                    &cmd_tx.0,
                    &mut keybinds_state,
                    &mut slash_writers.graphics,
                    &mut slash_writers.status_profile_open,
                    &mut slash_writers.hud_panels,
                    &mut slash_writers.net_status_visible,
                    &mut slash_writers.audio_mute,
                    &slash_writers.vana_clock,
                    &mut slash_writers.vana_clock_visible,
                    &mut slash_writers.sort_options,
                    &mut slash_writers.item_menu_focus,
                    &mut slash_writers.item_screen_container,
                    &dynamic_menu,
                    current_target,
                    self_pos,
                    &mut slash_writers.map_screen_state,
                    slash_writers.map_markers.reborrow(),
                    &slash_writers.map_view,
                    &slash_writers.minimap_state,
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
                    &mut slash_writers.item_screen_container,
                ) {
                    *mode = next;
                }
            }
            InputMode::PassiveCursor(state) => {
                if let Some(next) = handle_passive_cursor_key(
                    &ev.logical_key,
                    &bindings,
                    state,
                    &mut chat_scroll,
                    &mut slash_writers.active_chat_tab,
                    &scene_state,
                    &cmd_tx.0,
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
            InputMode::SubTarget(state) => {
                if let Some(next) = handle_sub_target_key(
                    &ev.logical_key,
                    &bindings,
                    state,
                    &mut scene_state,
                    &entities,
                    &cmd_tx.0,
                ) {
                    *mode = next;
                }
            }
            InputMode::DeliveryBox => {
                handle_delivery_key(
                    &ev.logical_key,
                    &bindings,
                    &mut slash_writers.delivery_state,
                    &mut scene_state,
                    &slash_writers.delivery_inv,
                    &cmd_tx.0,
                );
            }
            InputMode::Check => {
                if let Some(next) = handle_check_key(
                    &ev.logical_key,
                    &bindings,
                    &mut slash_writers.check_target,
                    &mut slash_writers.bazaar_state,
                    &scene_state,
                    &cmd_tx.0,
                ) {
                    *mode = next;
                }
            }
            InputMode::Bazaar => {
                if let Some(next) = handle_bazaar_key(
                    &ev.logical_key,
                    &bindings,
                    &mut slash_writers.bazaar_state,
                    &mut scene_state,
                    &cmd_tx.0,
                ) {
                    *mode = next;
                }
            }
            InputMode::Auction => {
                if let Some(next) = handle_auction_key(
                    &ev.logical_key,
                    &bindings,
                    &mut slash_writers.auction_state,
                    &mut scene_state,
                    &slash_writers.auction_inv,
                    &cmd_tx.0,
                ) {
                    *mode = next;
                }
            }
        }
    }
}

fn death_menu_response_command(
    offer: kuluu_snapshot::DeathMenuOffer,
    accept: bool,
) -> AgentCommand {
    let kind = match offer {
        kuluu_snapshot::DeathMenuOffer::Raise => ActionKind::RaiseMenu { accept },
        kuluu_snapshot::DeathMenuOffer::Tractor => ActionKind::TractorMenu { accept },
    };
    AgentCommand::Action {
        target_id: 0,
        target_index: 0,
        kind,
    }
}

#[cfg(test)]
mod death_menu_tests {
    use super::*;
    use kuluu_snapshot::DeathMenuOffer;

    #[test]
    fn raise_offer_dispatches_the_existing_raise_reply_action() {
        let cmd = death_menu_response_command(DeathMenuOffer::Raise, true);
        assert!(matches!(
            cmd,
            AgentCommand::Action {
                target_id: 0,
                target_index: 0,
                kind: ActionKind::RaiseMenu { accept: true },
            }
        ));
    }

    #[test]
    fn tractor_offer_dispatches_the_existing_tractor_reply_action() {
        let cmd = death_menu_response_command(DeathMenuOffer::Tractor, false);
        assert!(matches!(
            cmd,
            AgentCommand::Action {
                target_id: 0,
                target_index: 0,
                kind: ActionKind::TractorMenu { accept: false },
            }
        ));
    }
}

pub fn dialog_mode_sync_system(
    state: Res<SceneState>,
    mut mode: ResMut<InputMode>,
    mut cursors: Local<DialogCursors>,
) {
    let dialog = state.snapshot.dialog.as_ref();
    match (&*mode, dialog.is_some()) {
        (InputMode::World, true) => *mode = InputMode::Dialog(DialogCursor::default()),
        (InputMode::Dialog(_), false) => {
            *mode = InputMode::World;
            cursors.closed();
        }
        _ => {}
    }
    let InputMode::Dialog(cursor) = &mut *mode else {
        return;
    };
    let first_row = dialog.and_then(default_grid_choice).unwrap_or_default();
    if let Some(row) = cursors.switch(dialog.map(frame_id), cursor.cursor, first_row) {
        cursor.cursor = row;
    }
}

/// Per-menu cursor memory. A submenu replaces the dialog frame in place while
/// Dialog mode stays active (the Mog Menu's Delivery Box row, the delivery
/// grid), so without this the cursor keeps the parent row's index — which is
/// why "Delivery Box" (row 2) opened onto "Send" (row 2) instead of "Receive".
/// Retail opens each menu on its first row and restores the row a menu was left
/// on when Esc backs out (artifacts/retail/moghouse-menu-notes.md).
#[derive(Default)]
pub struct DialogCursors {
    open: Option<u64>,
    seen: std::collections::HashMap<u64, u32>,
}

impl DialogCursors {
    /// Files `cursor` under the frame being left and returns the row the newly
    /// shown `frame` opens on — `None` while the frame is unchanged.
    fn switch(&mut self, frame: Option<u64>, cursor: u32, first_row: u32) -> Option<u32> {
        if frame == self.open {
            return None;
        }
        if let Some(left) = self.open {
            self.seen.insert(left, cursor);
        }
        self.open = frame;
        let frame = frame?;
        Some(self.seen.get(&frame).copied().unwrap_or(first_row))
    }

    fn closed(&mut self) {
        self.open = None;
        self.seen.clear();
    }
}

/// Which menu is on screen, for cursor bookkeeping. Deliberately blind to the
/// choice *labels*: the delivery grid rewrites its cell captions as slots fill
/// and stack counts change, and that must not read as a new menu.
fn frame_id(dialog: &kuluu_snapshot::DialogState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    dialog.event_id.hash(&mut hasher);
    dialog.npc_id.hash(&mut hasher);
    dialog.prompt.hash(&mut hasher);
    dialog.choices.len().hash(&mut hasher);
    dialog.text_entry.hash(&mut hasher);
    hasher.finish()
}

/// The choice index the cursor should default to for a grid dialog: the first
/// selectable grid cell (retail focuses the slot grid, not the surrounding
/// recipient / Cancel rows). `None` when the frame has no grid.
fn default_grid_choice(dialog: &kuluu_snapshot::DialogState) -> Option<u32> {
    dialog.grid.as_ref()?.cells.iter().find_map(|c| c.choice)
}

fn handle_trade_key(
    key: &Key,
    bindings: &Bindings,
    trade_state: &mut kuluu_render::hud::trade::TradeState,
    trade_intent: &mut MessageWriter<kuluu_render::hud::trade::TradeIntent>,
    scene_state: &mut SceneState,
) -> Option<InputMode> {
    use kuluu_render::hud::trade::{self, TradeFocus, TradeSelector};

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
                let snapshot_gil = kuluu_render::hud::delivery::current_gil(&scene_state.snapshot);
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

fn chat_buffer_for_mode(
    mode_idx: usize,
    target_ent: Option<&kuluu_snapshot::Entity>,
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

fn handle_chat_key(
    key: &Key,
    bindings: &Bindings,
    buffer: &mut ChatBuffer,
    history: &ChatHistory,
) -> ChatAction {
    if bindings.matches_logical(Action::ChatSubmit, key) {
        return ChatAction::Submit;
    }
    if bindings.matches_logical(Action::ChatExit, key) {
        return if buffer.text.is_empty() {
            ChatAction::Exit
        } else {
            *buffer = ChatBuffer::empty();
            ChatAction::Stay
        };
    }
    // Free while the bar is open: the movement/camera system early-returns on
    // InputMode::Chat, so ArrowUp/Down never reach CameraPitchUp/Down here.
    if bindings.matches_logical(Action::NavUp, key) {
        buffer.recall_older(history);
        return ChatAction::Stay;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        buffer.recall_newer(history);
        return ChatAction::Stay;
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
    entities: &[kuluu_snapshot::Entity],
    self_pos: kuluu_snapshot::Vec3,
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
    fishing_gate: kuluu_render::fishing_spot::FishingGate,
    slash_writers: &mut SlashWriters,
    draw_distance: &mut kuluu_render::dat_mzb::DrawDistance,
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
            slash_writers.chat_history.push(trimmed);
            if trimmed.starts_with('/') {
                let outcome = parse_slash(
                    trimmed,
                    entities,
                    self_pos,
                    current_target,
                    scene_state.snapshot.zone_id,
                    scene_state.snapshot.self_char_id,
                    &scene_state.snapshot.party,
                    scene_state.snapshot.myroom,
                    fishing_gate,
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
                    // `/check <pc>` opens the same window the Check menu entry
                    // does; the other check kinds answer in chat only.
                    SlashOutcome::Command(AgentCommand::CheckTarget {
                        target_id,
                        kind: kuluu_session::state::CheckKind::Check,
                        ..
                    }) if entities.iter().any(|e| {
                        e.id == *target_id && e.kind == kuluu_snapshot::EntityKind::Pc
                    }) =>
                    {
                        slash_writers.check_target.open(
                            *target_id,
                            kuluu_render::hud::check_view::wares_enabled(
                                &scene_state.snapshot,
                                *target_id,
                            ),
                        );
                        Some(InputMode::Check)
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

fn push_system_chat_line(scene_state: &mut SceneState, text: String) {
    scene_state.push_local_toast(system_chat_line(text));
}

fn push_local_chat_line(scene_state: &mut SceneState, kind: u8, text: String) {
    use kuluu_snapshot::{ChatChannel, ChatLine};
    let channel = match kind {
        0 => ChatChannel::Say,
        1 => ChatChannel::Shout,
        4 => ChatChannel::Party,
        5 => ChatChannel::Linkshell,
        0x1A => ChatChannel::Yell,
        k if k == ffxi_proto::map::chat_kind::EMOTION => ChatChannel::Emote,
        _ => ChatChannel::Other,
    };
    let sender = scene_state
        .snapshot
        .char_name
        .clone()
        .unwrap_or_else(|| "you".into());
    scene_state.push_local_toast(ChatLine {
        spans: Vec::new(),
        channel,
        sender,
        text,
        server_ts: 0,
        local_seq: 0,
    });
}

fn push_local_tell_echo(scene_state: &mut SceneState, to: String, text: String) {
    use kuluu_snapshot::{ChatChannel, ChatLine};
    scene_state.push_local_toast(ChatLine {
        spans: Vec::new(),
        channel: ChatChannel::Tell,
        sender: to,
        text,
        server_ts: 0,
        local_seq: 0,
    });
}

/// Menu actions that take retail's sub-target confirm step before firing
/// (spells/abilities/weaponskills/items). Everything else — move, equip, emote,
/// and self-only spells/abilities (validTarget SELF, e.g. Boost) — dispatches
/// immediately; dispatch_dynamic_menu_action routes the self-only ones to <me>.
/// (vendor/server/sql/{abilities,spell_list}.sql validTarget, TARGET_SELF=0x01.)
fn sub_target_action_for(
    action: kuluu_render::hud::menu::DynamicMenuAction,
) -> Option<kuluu_render::input_mode::SubTargetAction> {
    use kuluu_render::hud::menu::DynamicMenuAction as A;
    use kuluu_render::input_mode::SubTargetAction as S;
    match action {
        A::CastSpell { spell_id }
            if ffxi_vocab::valid_target::spell(spell_id).is_some_and(|f| f.is_self_only()) =>
        {
            None
        }
        A::JobAbility { ability_id } | A::PetAbility { ability_id }
            if ffxi_vocab::valid_target::ability(ability_id).is_some_and(|f| f.is_self_only()) =>
        {
            None
        }
        A::CastSpell { spell_id } => Some(S::Spell(spell_id)),
        A::JobAbility { ability_id } | A::PetAbility { ability_id } => Some(S::Ability(ability_id)),
        A::Weaponskill { skill_id } => Some(S::WeaponSkill(skill_id)),
        A::RangedAttack => Some(S::Ranged),
        // Both act on the player, so neither opens the sub-target cursor.
        A::Dismount | A::ChocoboDig => None,
        A::UseItem {
            container,
            index,
            item_no,
        } => Some(S::Item {
            container,
            index,
            item_no,
        }),
        A::MoveItem { .. } => None,
        A::OpenItemAction { .. } => None,
        A::EquipItem { .. } => None,
        A::KeyItem { .. } => None,
        A::Emote { .. } => None,
    }
}

/// Inverse of `sub_target_action_for`, used to fire the pending action once
/// the sub-target cursor is confirmed. Job vs pet ability collapses to
/// JobAbility; their dispatch is identical.
fn dynamic_action_for(
    action: kuluu_render::input_mode::SubTargetAction,
) -> kuluu_render::hud::menu::DynamicMenuAction {
    use kuluu_render::hud::menu::DynamicMenuAction as A;
    use kuluu_render::input_mode::SubTargetAction as S;
    match action {
        S::Spell(spell_id) => A::CastSpell { spell_id },
        S::Ability(ability_id) => A::JobAbility { ability_id },
        S::WeaponSkill(skill_id) => A::Weaponskill { skill_id },
        S::Ranged => A::RangedAttack,
        S::Item {
            container,
            index,
            item_no,
        } => A::UseItem {
            container,
            index,
            item_no,
        },
    }
}

/// Per-frame snapshot of targetable entities for sub-target candidate
/// selection (kuluu-render::sub_target owns the pure logic).
fn gather_sub_target_entities(
    scene_state: &SceneState,
) -> Vec<kuluu_render::sub_target::SubTargetEntity> {
    use kuluu_snapshot::EntityKind;
    let snap = &scene_state.snapshot;
    let self_id = snap.self_char_id;
    let self_pos = snap.self_pos.pos;
    snap.entities
        .iter()
        .map(|e| {
            let dx = e.pos.x - self_pos.x;
            let dy = e.pos.y - self_pos.y;
            let dz = e.pos.z - self_pos.z;
            let is_party = snap.party.iter().any(|m| m.id == e.id);
            kuluu_render::sub_target::SubTargetEntity {
                id: e.id,
                is_self: Some(e.id) == self_id,
                is_pc: matches!(e.kind, EntityKind::Pc),
                is_party,
                // Alliance membership is not surfaced in the wire snapshot
                // yet; party covers the common case (kuluu: revisit when
                // alliance lists land).
                is_alliance: is_party,
                is_enemy: matches!(e.kind, EntityKind::Mob),
                is_npc: matches!(e.kind, EntityKind::Npc),
                is_dead: e.hp_pct == Some(0),
                dist_sq: dx * dx + dy * dy + dz * dz,
            }
        })
        .collect()
}

/// Open the retail sub-target confirm step for `action`. Returns None (stay
/// in the current mode) when nothing in range qualifies, echoing retail's
/// refusal line.
/// Retail: with a valid target already selected, confirming an action casts on it
/// immediately — the flashing sub-target cursor is only for choosing a *different*
/// target. True when `current_target` satisfies the action's validTarget flags, so
/// the caller dispatches directly instead of opening the cursor. (Self-only actions
/// never reach here — sub_target_action_for already routed them to <me>.)
fn selected_target_valid(
    action: kuluu_render::input_mode::SubTargetAction,
    current_target: Option<u32>,
    scene_state: &SceneState,
) -> bool {
    use kuluu_render::sub_target;
    let Some(tid) = current_target else {
        return false;
    };
    let flags = sub_target::action_flags(action);
    gather_sub_target_entities(scene_state)
        .iter()
        .any(|e| e.id == tid && sub_target::entity_valid(flags, e))
}

fn open_sub_target(
    action: kuluu_render::input_mode::SubTargetAction,
    current_target: Option<u32>,
    scene_state: &mut SceneState,
    return_to: InputMode,
) -> Option<InputMode> {
    use kuluu_render::sub_target;
    let flags = sub_target::action_flags(action);
    let ents = gather_sub_target_entities(scene_state);
    let Some(candidate) = sub_target::initial_candidate(flags, current_target, &ents) else {
        push_system_chat_line(scene_state, "Unable to see any qualified targets.".into());
        return None;
    };
    let mut st = kuluu_render::input_mode::SubTargetState::open(action, flags.0, return_to);
    st.candidate = Some(candidate);
    Some(InputMode::SubTarget(st))
}

/// Retail sub-target cursor keys: Tab/arrows cycle valid candidates in
/// distance order, Enter fires the pending action at the candidate, Esc
/// returns to the originating menu with its cursor preserved.
fn handle_sub_target_key(
    key: &Key,
    bindings: &Bindings,
    state: &mut kuluu_render::input_mode::SubTargetState,
    scene_state: &mut SceneState,
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    use ffxi_vocab::valid_target::TargetFlags;
    use kuluu_render::sub_target;

    let flags = TargetFlags(state.flags);
    let ents = gather_sub_target_entities(scene_state);

    // Entities move and die while the cursor is up; re-park on the nearest
    // valid candidate if ours stopped qualifying.
    if let Some(id) = state.candidate {
        let still_valid = ents
            .iter()
            .any(|e| e.id == id && sub_target::entity_valid(flags, e));
        if !still_valid {
            state.candidate = sub_target::initial_candidate(flags, None, &ents);
        }
    }

    let forward = bindings.matches_logical(Action::CycleTarget, key)
        || bindings.matches_logical(Action::NavDown, key)
        || bindings.matches_logical(Action::NavRight, key);
    let reverse = bindings.matches_logical(Action::NavUp, key)
        || bindings.matches_logical(Action::NavLeft, key);
    if forward || reverse {
        state.candidate = sub_target::cycle_candidate(flags, state.candidate, &ents, reverse);
        return None;
    }

    if bindings.matches_logical(Action::NavConfirm, key)
        || bindings.matches_logical(Action::ConfirmAction, key)
    {
        let Some(id) = state.candidate else {
            push_system_chat_line(scene_state, "Unable to see any qualified targets.".into());
            return None;
        };
        let self_pos = scene_state.snapshot.self_pos.pos;
        dispatch_dynamic_menu_action(
            dynamic_action_for(state.action),
            Some(id),
            self_pos,
            entities,
            cmd_tx,
            scene_state,
        );
        return Some(InputMode::World);
    }

    if bindings.matches_logical(Action::NavCancel, key) {
        return Some((*state.return_to).clone());
    }
    None
}

fn dispatch_dynamic_menu_action(
    action: kuluu_render::hud::menu::DynamicMenuAction,
    target_id: Option<u32>,
    self_pos: kuluu_snapshot::Vec3,
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
    scene_state: &mut SceneState,
) {
    use kuluu_render::hud::menu::DynamicMenuAction as A;
    // Refuse an ability still on recast client-side (retail blocks it locally rather
    // than sending it and getting the server's "wait longer" reject).
    let now_unix = kuluu_snapshot::recast_now_unix();
    if let Some(remaining) = kuluu_render::hud::menu::action_recast_remaining(
        &scene_state.snapshot.ability_recasts,
        &action,
        now_unix,
    ) {
        push_system_chat_line(
            scene_state,
            format!(
                "Unable to use that ability. ({} remaining)",
                kuluu_render::hud::format_timer(remaining)
            ),
        );
        return;
    }
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
                ffxi_vocab::valid_target::spell(spell_id).is_some_and(|f| f.is_self_only());
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
                ffxi_vocab::valid_target::ability(ability_id).is_some_and(|f| f.is_self_only());
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
        A::RangedAttack => {
            let Some((tid, tidx)) = pick_target(true) else {
                push_system_chat_line(
                    scene_state,
                    "[menu] ranged attack: requires a battle target".into(),
                );
                return;
            };
            (
                "ranged",
                AgentCommand::Action {
                    target_id: tid,
                    target_index: tidx,
                    kind: ActionKind::Shoot,
                },
            )
        }
        A::Dismount | A::ChocoboDig => {
            let Some((tid, tidx)) = self_target() else {
                push_system_chat_line(scene_state, "[menu] mount: self not resolved yet".into());
                return;
            };
            let (label, kind) = if matches!(action, A::Dismount) {
                ("dismount", ActionKind::Dismount)
            } else {
                ("chocobo dig", ActionKind::ChocoboDig)
            };
            (
                label,
                AgentCommand::Action {
                    target_id: tid,
                    target_index: tidx,
                    kind,
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
        A::MoveItem {
            quantity,
            from_container,
            from_slot,
            to_container,
            item_no: _,
        } => (
            "moveitem",
            AgentCommand::MoveItem {
                quantity,
                from_container,
                to_container,
                from_slot,
                to_slot: None,
            },
        ),
        A::Emote { emote_id } => {
            use ffxi_proto::map::emote;
            // Untargeted unless something is selected (UniqueNo/ActIndex 0).
            let target = target_id.and_then(|id| entities.iter().find(|e| e.id == id));
            let param = match emote_id {
                id if id == emote::BELL => emote::BELL_NOTE_MIN,
                id if id == emote::JOB => {
                    let main_job = scene_state
                        .snapshot
                        .self_char_id
                        .and_then(|id| scene_state.snapshot.party.iter().find(|m| m.id == id))
                        .map(|m| m.main_job)
                        .unwrap_or(0);
                    if main_job == 0 {
                        push_system_chat_line(scene_state, "[menu] jobemote: job unknown".into());
                        return;
                    }
                    emote::JOB_PARAM_BASE + (main_job as u16 - 1)
                }
                _ => 0,
            };
            (
                "emote",
                AgentCommand::Emote {
                    emote_id,
                    mode: emote::mode::ALL,
                    param,
                    target_id: target.map(|e| e.id),
                    target_index: target.map(|e| e.act_index),
                },
            )
        }
        // Pushed as a submenu by confirm_menu_at_cursor, never dispatched.
        A::OpenItemAction { .. } => return,
        // Handled in confirm_menu_at_cursor (chat echo, menu stays open).
        A::KeyItem { .. } => return,
        A::EquipItem {
            container,
            container_index,
            equip_slot,
            item_no,
        } => {
            let already_equipped = scene_state
                .snapshot
                .equipped
                .get(equip_slot as usize)
                .copied()
                .flatten()
                == Some(item_no);
            if already_equipped {
                // Re-selecting the item already in this slot toggles it off.
                // LSB unequips when slotID (container_index) is 0, regardless of
                // container: vendor/server/src/map/utils/charutils.cpp:3147
                // ("slotID of zero = unequip"). LOC_INVENTORY (0) always passes
                // the equip_set container validation.
                (
                    "unequip",
                    AgentCommand::Equip {
                        container: 0,
                        container_index: 0,
                        equip_slot,
                    },
                )
            } else {
                (
                    "equip",
                    AgentCommand::Equip {
                        container,
                        container_index,
                        equip_slot,
                    },
                )
            }
        }
    };
    if let Err(e) = cmd_tx.try_send(cmd) {
        push_system_chat_line(scene_state, format!("[menu] {kind_name} dropped: {e}"));
    }
}

/// Sends the choice to the session; returns the container to browse when the
/// choice was a Mog Menu storage row (the session closes the menu from the same
/// choice, the viewer opens its Items window on the bag).
fn confirm_dialog_choice(
    choice: u32,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<u8> {
    let mut open_storage = None;
    if let Some(d) = scene_state.snapshot.dialog.as_ref() {
        // A server customMenu answers with a `_CUSTOM_MENU` tell, not an
        // EndEventChoice — the server owns the context, not an event.
        if d.custom_menu {
            let _ = cmd_tx.try_send(AgentCommand::CustomMenuRespond {
                title: d.prompt.clone().unwrap_or_default(),
                option: d.choices.get(choice as usize).cloned(),
            });
            return None;
        }
        open_storage = mog_menu_storage_choice(d, choice);
        // EVENT_END validates against the event id, which the trigger carries in
        // EventPara (event_num is the zone) — see event_trigger in session/mod.rs.
        let _ = cmd_tx.try_send(AgentCommand::EndEventChoice {
            event_id: d.npc_id,
            act_index: d.act_index,
            event_num: d.event_para,
            choice,
        });
    }
    open_storage
}

fn mog_menu_storage_choice(d: &kuluu_snapshot::DialogState, choice: u32) -> Option<u8> {
    use kuluu_session::local_menu::{storage_row_container, MOG_MENU_ID, STORAGE_PROMPT};
    if d.npc_id != MOG_MENU_ID {
        return None;
    }
    // Storage rows only exist inside the Storage submenu; the prompt check keeps
    // the root menu's "Storage" row (which opens that submenu) from matching.
    if d.prompt.as_deref() != Some(STORAGE_PROMPT) {
        return None;
    }
    storage_row_container(d.choices.get(choice as usize)?.as_str())
}

/// The Items window opened directly on `container` (from a Mog Menu storage row).
fn open_items_on_bag(
    container: u8,
    item_bag: &mut kuluu_render::hud::item_screen::ItemScreenContainer,
) -> InputMode {
    item_bag.0 = container;
    let mut stack = MenuStack::root();
    stack.push(MenuKind::Items);
    InputMode::Menu(stack)
}

fn confirm_quick_action_at_cursor(
    state: &QuickActionState,
    scene_state: &mut SceneState,
    target_id: Option<u32>,
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let label = kuluu_render::hud::quick_action::entry_label(state.has_target, state.cursor);
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

/// The mouse-activation streams `mouse_nav_dispatch_system` consumes, bundled
/// to stay inside Bevy's 16-parameter system limit.
#[derive(SystemParam)]
pub struct MouseNavEvents<'w, 's> {
    pub menu: MessageReader<'w, 's, kuluu_render::hud::menu::MenuRowActivated>,
    pub dialog: MessageReader<'w, 's, kuluu_render::hud::dialog::DialogChoiceActivated>,
    pub quick_action: MessageReader<'w, 's, kuluu_render::hud::quick_action::QuickActionActivated>,
    pub target_action:
        MessageReader<'w, 's, kuluu_render::hud::target_action_menu::TargetActionActivated>,
    pub sort_req: MessageReader<'w, 's, kuluu_render::hud::item_detail::InventorySortRequested>,
    pub auction: MessageReader<'w, 's, kuluu_render::hud::auction::AuctionRowActivated>,
}

#[allow(clippy::too_many_arguments)]
pub fn mouse_nav_dispatch_system(
    mut events: MouseNavEvents,
    mut auction_screen: ResMut<kuluu_render::hud::auction::AuctionScreenState>,
    auction_inv: Res<kuluu_render::hud::auction::AuctionSellInventory>,
    cmd_tx: Res<CommandTx>,
    mut bindings: ResMut<Bindings>,
    mut keybinds_state: ResMut<KeybindsStateRes>,
    mut mode: ResMut<InputMode>,
    target: Res<Target>,
    mut scene_state: ResMut<SceneState>,
    mut menu_writers: MenuConfirmWriters,
    dynamic_menu: Res<kuluu_render::hud::menu::DynamicMenu>,
    mut check_target: ResMut<kuluu_render::hud::check_view::CheckTarget>,
    mut trade_state: ResMut<kuluu_render::hud::trade::TradeState>,
    mut select_target: ResMut<SelectTargetMode>,
) {
    let entities = scene_state.snapshot.entities.clone();
    let current_target = target.id;
    let self_pos = scene_state.snapshot.self_pos.pos;

    for ev in events.menu.read() {
        if let InputMode::Menu(stack) = &mut *mode {
            // A click drops the cursor on the clicked row of the current level,
            // then confirms — same as pressing Enter there.
            if let Some(level) = stack.current_mut() {
                level.cursor = ev.slot;
            }
            if let Some(next) = confirm_menu_at_cursor(
                &mut bindings,
                stack,
                &mut scene_state,
                &cmd_tx.0,
                &mut keybinds_state,
                &mut menu_writers.graphics,
                &mut menu_writers.status_profile_open,
                &mut menu_writers.hud_panels,
                &mut menu_writers.net_status,
                &mut menu_writers.audio_mute,
                &menu_writers.vana_clock,
                &mut menu_writers.vana_clock_visible,
                &dynamic_menu,
                current_target,
                self_pos,
            ) {
                *mode = next;
            }
        }
    }

    for ev in events.dialog.read() {
        if let InputMode::Dialog(cursor) = &mut *mode {
            // Text-entry frames have no clickable choices; typing owns the frame.
            if scene_state
                .snapshot
                .dialog
                .as_ref()
                .is_some_and(|d| d.text_entry)
            {
                continue;
            }
            cursor.cursor = ev.choice;
            if let Some(container) = confirm_dialog_choice(ev.choice, &mut scene_state, &cmd_tx.0) {
                *mode = open_items_on_bag(container, &mut menu_writers.item_screen_container);
            }
        }
    }

    for ev in events.quick_action.read() {
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

    for ev in events.target_action.read() {
        if let InputMode::TargetAction(state) = &mut *mode {
            state.cursor = ev.slot;

            let entries = kuluu_render::hud::overlay::RETAIL.resolve_target_actions(&state.ctx);
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

    for ev in events.sort_req.read() {
        if let Err(e) = cmd_tx.0.try_send(AgentCommand::StackInventory {
            container: ev.container,
        }) {
            push_system_chat_line(&mut scene_state, format!("sort dropped (channel): {e}"));
        }
    }

    for ev in events.auction.read() {
        if matches!(*mode, InputMode::Auction) {
            if let Some(next) = auction_click(
                ev.region,
                ev.slot,
                &mut auction_screen,
                &mut scene_state,
                &auction_inv,
                &cmd_tx.0,
            ) {
                *mode = next;
            }
        }
    }
}

fn handle_dialog_key(
    key: &Key,
    bindings: &Bindings,
    cursor: &mut DialogCursor,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    item_bag: &mut kuluu_render::hud::item_screen::ItemScreenContainer,
) -> Option<InputMode> {
    // Free-text frame (delivery-box recipient prompt): edit a line buffer and
    // answer with TextInput; characters must not fall through to nav bindings.
    if scene_state
        .snapshot
        .dialog
        .as_ref()
        .is_some_and(|d| d.text_entry)
    {
        let entry = cursor.entry.get_or_insert_with(String::new);
        if bindings.matches_logical(Action::ChatSubmit, key) {
            let text = std::mem::take(entry);
            cursor.entry = None;
            let _ = cmd_tx.try_send(AgentCommand::TextInput { text });
            return None;
        }
        if bindings.matches_logical(Action::ChatExit, key) {
            // Esc: retail closes the name-entry box back to the Send panel;
            // an empty answer clears any staged recipient and re-renders it.
            cursor.entry = None;
            let _ = cmd_tx.try_send(AgentCommand::TextInput {
                text: String::new(),
            });
            return None;
        }
        if bindings.matches_logical(Action::ChatBackspace, key) {
            entry.pop();
            return None;
        }
        match key {
            Key::Space => entry.push(' '),
            Key::Character(s) => {
                for c in s.chars() {
                    if !c.is_control() {
                        entry.push(c);
                    }
                }
            }
            _ => {}
        }
        return None;
    }
    cursor.entry = None;

    // Plain speech (no choices) clamps to 0 and still confirms/advances on Enter.
    let max_index = scene_state
        .snapshot
        .dialog
        .as_ref()
        .map(|d| d.choices.len() as u32)
        .unwrap_or(0)
        .min(kuluu_render::hud::dialog::MAX_OPTION_ROWS)
        .saturating_sub(1);
    let grid = scene_state
        .snapshot
        .dialog
        .as_ref()
        .and_then(|d| d.grid.clone());
    let nav_delta = if bindings.matches_logical(Action::NavUp, key) {
        Some((0i32, -1i32))
    } else if bindings.matches_logical(Action::NavDown, key) {
        Some((0, 1))
    } else if bindings.matches_logical(Action::NavLeft, key) {
        Some((-1, 0))
    } else if bindings.matches_logical(Action::NavRight, key) {
        Some((1, 0))
    } else {
        None
    };
    if let Some((dx, dy)) = nav_delta {
        cursor.cursor = match &grid {
            // Delivery-box style panel: the cursor walks the 2x4 icon grid
            // itself (retail behavior), with any pre-grid rows (recipient)
            // above and post-grid rows (Cancel) below.
            Some(grid) => grid_nav_choice(grid, max_index, cursor.cursor, dx, dy),
            None => match dy {
                -1 => cursor.cursor.saturating_sub(1),
                1 => (cursor.cursor + 1).min(max_index),
                _ => cursor.cursor,
            },
        };
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        if let Some(container) =
            confirm_dialog_choice(cursor.cursor.min(max_index), scene_state, cmd_tx)
        {
            return Some(open_items_on_bag(container, item_bag));
        }
        return None;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        // A server customMenu cancels with a "Canceled." `_CUSTOM_MENU` tell
        // (its onCancelled branch); a plain EndEvent would leave it dangling.
        if let Some(d) = scene_state
            .snapshot
            .dialog
            .as_ref()
            .filter(|d| d.custom_menu)
        {
            let _ = cmd_tx.try_send(AgentCommand::CustomMenuRespond {
                title: d.prompt.clone().unwrap_or_default(),
                option: None,
            });
            return None;
        }
        // Reconcile via the session snapshot; clearing here flickers multi-frame events.
        let _ = cmd_tx.try_send(AgentCommand::EndEvent);
        return None;
    }
    None
}

/// Spatial cursor movement over a [`kuluu_snapshot::DialogGrid`]: choices
/// referenced by grid cells are navigated as a 2D grid (nearest-column rule on
/// row changes), while choices before/after the grid's range (recipient row,
/// Cancel) behave as flat rows above/below it. Returns the new choice index
/// (unchanged when the move has nowhere to go, like retail).
fn grid_nav_choice(
    grid: &kuluu_snapshot::DialogGrid,
    max_index: u32,
    cur: u32,
    dx: i32,
    dy: i32,
) -> u32 {
    let cols = i32::from(grid.cols.max(1));
    // Selectable cells as (x, y, choice).
    let sel: Vec<(i32, i32, u32)> = grid
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            c.choice
                .map(|ch| ((i as i32) % cols, (i as i32) / cols, ch))
        })
        .collect();
    let grid_min = sel.iter().map(|&(_, _, c)| c).min();
    let grid_max = sel.iter().map(|&(_, _, c)| c).max();

    // Nearest selectable cell to column `x` on row `y` (if the row has any).
    let cell_on_row = |y: i32, x: i32| -> Option<u32> {
        sel.iter()
            .filter(|&&(_, cy, _)| cy == y)
            .min_by_key(|&&(cx, _, _)| (cx - x).abs())
            .map(|&(_, _, c)| c)
    };
    // First selectable row scanning from `y` in `dir`, exclusive.
    let next_row = |y: i32, dir: i32| -> Option<i32> {
        let mut ny = y + dir;
        while (0..i32::from(grid.rows.max(1))).contains(&ny) {
            if sel.iter().any(|&(_, cy, _)| cy == ny) {
                return Some(ny);
            }
            ny += dir;
        }
        None
    };

    if let Some(&(x, y, _)) = sel.iter().find(|&&(_, _, c)| c == cur) {
        if dx != 0 {
            // Stay on the row; step to the nearest selectable cell that way.
            return sel
                .iter()
                .filter(|&&(cx, cy, _)| cy == y && (cx - x).signum() == dx)
                .min_by_key(|&&(cx, _, _)| (cx - x).abs())
                .map_or(cur, |&(_, _, c)| c);
        }
        return match next_row(y, dy) {
            Some(ny) => cell_on_row(ny, x).unwrap_or(cur),
            // Off the top: pre-grid rows (recipient). Off the bottom:
            // post-grid rows (Cancel).
            None if dy < 0 => grid_min.filter(|&m| m > 0).map_or(cur, |m| m - 1),
            None => grid_max
                .filter(|&m| m < max_index)
                .map_or(cur, |m| (m + 1).min(max_index)),
        };
    }

    // Cursor sits on a flat row outside the grid.
    let before_grid = grid_min.is_some_and(|m| cur < m);
    match (dy, before_grid) {
        // Down from the pre-grid rows: into the grid once we run out of them,
        // otherwise the next flat row.
        (1, true) => {
            if grid_min == Some(cur + 1) {
                next_row(-1, 1)
                    .and_then(|y| cell_on_row(y, 0))
                    .unwrap_or((cur + 1).min(max_index))
            } else {
                (cur + 1).min(max_index)
            }
        }
        // Up from the post-grid rows: back into the grid's bottom row.
        (-1, false) => {
            if grid_max == Some(cur.saturating_sub(1)) && cur > 0 {
                next_row(i32::from(grid.rows.max(1)), -1)
                    .and_then(|y| cell_on_row(y, 0))
                    .unwrap_or(cur - 1)
            } else {
                cur.saturating_sub(1)
            }
        }
        (-1, true) => cur.saturating_sub(1),
        (1, false) => (cur + 1).min(max_index),
        _ => cur,
    }
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
    target: Option<&kuluu_snapshot::Entity>,
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
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let entry_count = kuluu_render::hud::quick_action::entry_count(state.has_target);
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

const CHAT_SCROLL_PAGE_ROWS: usize = 8;

/// Drives the "active window" cursor (retail's Select-active-window / F key).
/// F steps focus across the on-screen windows; within the focused window the
/// Nav keys scroll/select and confirm/cancel act on it.
fn handle_passive_cursor_key(
    key: &Key,
    bindings: &Bindings,
    state: &mut kuluu_render::input_mode::PassiveCursorState,
    chat_scroll: &mut ChatScroll,
    active_chat_tab: &mut ActiveChatTab,
    scene_state: &SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    use kuluu_render::input_mode::{PassiveCursorFocus, PassiveCursorState};

    let icons = &scene_state.snapshot.status_icons;

    // F advances focus across windows: Chat -> StatusIcons (when buffs exist)
    // -> World (unfocused), matching retail's window-change cycle.
    if bindings.matches_logical(Action::SelectActiveWindow, key) {
        return Some(match state.focus {
            PassiveCursorFocus::Chat if !icons.is_empty() => {
                InputMode::PassiveCursor(PassiveCursorState::fresh_status())
            }
            _ => InputMode::World,
        });
    }

    match state.focus {
        PassiveCursorFocus::Chat => {
            let max_back = kuluu_render::snapshot::rendered_chat(scene_state).len();
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
                let next = chat_scroll.rows.saturating_add(CHAT_SCROLL_PAGE_ROWS);
                chat_scroll.rows = next.min(max_back.saturating_sub(1));
                return None;
            }
            if bindings.matches_logical(Action::PageDown, key) {
                chat_scroll.rows = chat_scroll.rows.saturating_sub(CHAT_SCROLL_PAGE_ROWS);
                return None;
            }
            // Left/Right cycle which chat tab the focused log shows.
            if bindings.matches_logical(Action::NavLeft, key) {
                active_chat_tab.0 = active_chat_tab.0.cycle_prev();
                return None;
            }
            if bindings.matches_logical(Action::NavRight, key) {
                active_chat_tab.0 = active_chat_tab.0.cycle_next();
                return None;
            }
            // Confirm expands the log to full-screen; cancel contracts it,
            // then a second cancel releases focus (retail's log window).
            if bindings.matches_logical(Action::NavConfirm, key) {
                state.chat_expanded = true;
                return None;
            }
            if bindings.matches_logical(Action::NavCancel, key) {
                if state.chat_expanded {
                    state.chat_expanded = false;
                    return None;
                }
                return Some(InputMode::World);
            }
            None
        }
        PassiveCursorFocus::StatusIcons => {
            if icons.is_empty() {
                return Some(InputMode::World);
            }
            let last = icons.len() - 1;
            state.status_cursor = state.status_cursor.min(last);
            const ROW: usize = kuluu_render::hud::status_ribbon::ICONS_PER_ROW;
            if bindings.matches_logical(Action::NavLeft, key) {
                state.status_cursor = state.status_cursor.saturating_sub(1);
                return None;
            }
            if bindings.matches_logical(Action::NavRight, key) {
                state.status_cursor = (state.status_cursor + 1).min(last);
                return None;
            }
            if bindings.matches_logical(Action::NavUp, key) {
                state.status_cursor = state.status_cursor.saturating_sub(ROW);
                return None;
            }
            if bindings.matches_logical(Action::NavDown, key) {
                state.status_cursor = (state.status_cursor + ROW).min(last);
                return None;
            }
            if bindings.matches_logical(Action::NavConfirm, key) {
                if let Some(&icon) = icons.get(state.status_cursor) {
                    if ffxi_vocab::status_effects::is_cancelable(icon) {
                        let _ = cmd_tx.try_send(AgentCommand::CancelBuff { icon });
                    }
                }
                return None;
            }
            if bindings.matches_logical(Action::NavCancel, key) {
                return Some(InputMode::World);
            }
            None
        }
    }
}

#[cfg(test)]
mod dialog_cursor_tests {
    use super::*;

    const NO_GRID: u32 = 0;
    const MOG_ROOT: u64 = 1;
    const DELIVERY_SUBMENU: u64 = 2;

    /// Picking "Delivery Box" (row 2 of the Mog Menu) must open the
    /// Receive/Send submenu on Receive, not carry row 2 into it and land on
    /// Send.
    #[test]
    fn a_submenu_opens_on_its_first_row() {
        let mut cursors = DialogCursors::default();
        assert_eq!(cursors.switch(Some(MOG_ROOT), 0, NO_GRID), Some(0));
        assert_eq!(
            cursors.switch(Some(DELIVERY_SUBMENU), 1, NO_GRID),
            Some(0),
            "the parent's row does not follow us in"
        );
    }

    /// ...and backing out puts the parent's cursor back where it was.
    #[test]
    fn backing_out_restores_the_parent_row() {
        let mut cursors = DialogCursors::default();
        cursors.switch(Some(MOG_ROOT), 0, NO_GRID);
        cursors.switch(Some(DELIVERY_SUBMENU), 1, NO_GRID);
        assert_eq!(
            cursors.switch(Some(MOG_ROOT), 0, NO_GRID),
            Some(1),
            "Delivery Box is still the highlighted root row"
        );
    }

    /// A redraw of the same frame (delivery slots filling in) must not move the
    /// cursor the player put somewhere.
    #[test]
    fn an_unchanged_frame_leaves_the_cursor_alone() {
        let mut cursors = DialogCursors::default();
        cursors.switch(Some(DELIVERY_SUBMENU), 0, NO_GRID);
        assert_eq!(cursors.switch(Some(DELIVERY_SUBMENU), 2, NO_GRID), None);
    }

    /// Closing the dialog forgets everything: the next conversation starts
    /// fresh rather than reopening on a stale row.
    #[test]
    fn closing_the_dialog_clears_the_memory() {
        let mut cursors = DialogCursors::default();
        cursors.switch(Some(MOG_ROOT), 0, NO_GRID);
        cursors.switch(Some(DELIVERY_SUBMENU), 3, NO_GRID);
        cursors.closed();
        assert_eq!(cursors.switch(Some(DELIVERY_SUBMENU), 0, NO_GRID), Some(0));
    }

    /// A grid frame opens on its first cell, not row 0.
    #[test]
    fn a_grid_frame_opens_on_its_first_cell() {
        const FIRST_CELL: u32 = 1;
        let mut cursors = DialogCursors::default();
        assert_eq!(
            cursors.switch(Some(DELIVERY_SUBMENU), 0, FIRST_CELL),
            Some(FIRST_CELL)
        );
    }
}

#[cfg(test)]
mod quick_action_tests {
    use super::*;
    use kuluu_snapshot::{Entity as WireEntity, EntityKind, Vec3 as WireVec3};

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
            name_vis: None,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: Default::default(),
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
mod chat_history_tests {
    use super::*;

    #[test]
    fn chat_arrows_page_the_submitted_line_history() {
        let bindings = Bindings::default();
        let mut history = ChatHistory::default();
        history.push("/heal");
        history.push("/tell Zilart hi");

        let mut buffer = ChatBuffer::empty();
        buffer.text.push_str("draft");

        for (key, expected) in [
            (Key::ArrowUp, "/tell Zilart hi"),
            (Key::ArrowUp, "/heal"),
            (Key::ArrowDown, "/tell Zilart hi"),
            (Key::ArrowDown, "draft"),
        ] {
            handle_chat_key(&key, &bindings, &mut buffer, &history);
            assert_eq!(buffer.text, expected, "after {key:?}");
        }
    }

    #[test]
    fn chat_typing_is_unaffected_by_the_history_bindings() {
        let bindings = Bindings::default();
        let history = ChatHistory::default();
        let mut buffer = ChatBuffer::empty();

        for key in [
            Key::Character("h".into()),
            Key::Character("i".into()),
            Key::Space,
            Key::ArrowUp,
            Key::Character("t".into()),
        ] {
            handle_chat_key(&key, &bindings, &mut buffer, &history);
        }

        assert_eq!(buffer.text, "hi t");
    }

    #[test]
    fn clearing_a_recalled_line_resets_the_history_cursor() {
        let bindings = Bindings::default();
        let mut history = ChatHistory::default();
        history.push("/heal");

        let mut buffer = ChatBuffer::empty();
        handle_chat_key(&Key::ArrowUp, &bindings, &mut buffer, &history);
        assert_eq!(buffer.history_pos, Some(0));

        handle_chat_key(&Key::Escape, &bindings, &mut buffer, &history);
        assert_eq!(buffer.text, "");
        assert_eq!(buffer.history_pos, None);

        handle_chat_key(&Key::ArrowUp, &bindings, &mut buffer, &history);
        assert_eq!(buffer.text, "/heal");
    }
}
