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

mod event_map;
pub use event_map::event_map_sync_system;

mod auto_enter;
pub use auto_enter::auto_enter_cs_system;

mod shop;
use shop::handle_shop_key;
pub use shop::{shop_mode_sync_system, shop_mouse_activate_system};

mod map_screen;

mod menu;
use menu::{confirm_menu_at_cursor, handle_menu_key};

mod slash_apply;
use slash_apply::apply_slash_outcome;

mod target_action;
use target_action::{
    answer_dismount_confirm, confirm_target_action_at_cursor, handle_target_action_key,
    handle_world_key,
};

/// The one key map for every amount the game asks for — auction price, shop and
/// bazaar quantity, delivery quantity and gil. Up/Down step the active digit,
/// Left/Right move the column. Taking the whole amount is the All column at the
/// left end of that walk, not a separate key: retail draws it as a column
/// (.agents/skills/retail-observe/references/auction-house.md "Price Set").
fn spinner_nav(
    spinner: &mut kuluu_render::hud::digit_spinner::DigitSpinner,
    key: &Key,
    bindings: &Bindings,
) {
    if bindings.matches_logical(Action::NavUp, key) {
        spinner.up();
    } else if bindings.matches_logical(Action::NavDown, key) {
        spinner.down();
    } else if bindings.matches_logical(Action::NavLeft, key) {
        spinner.left();
    } else if bindings.matches_logical(Action::NavRight, key) {
        spinner.right();
    }
}

#[derive(Resource, Default)]
pub struct CaptureMode {
    pub active: bool,

    pub restore_limiter: Option<bevy_framepace::Limiter>,
}

#[derive(SystemParam)]
pub struct SlashWriters<'w, 's> {
    pub command_surface: Res<'w, crate::view_native::command_surface::CommandSurface>,
    pub load_mmb: MessageWriter<'w, LoadMmbRequest>,
    pub load_mzb: MessageWriter<'w, LoadMzbRequest>,
    pub set_sub_area: MessageWriter<'w, kuluu_render::sub_area_activation::SetSubArea>,
    pub debug_heights: MessageWriter<'w, DebugHeightsRequest>,

    #[cfg(feature = "enhanced-shutdown-counter")]
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

    /// Disengage releases the camera lock the same as the H toggle; the slash
    /// and menu paths both funnel through here, so the writer rides the bundle
    /// (text_input_system is at the 16-param cap on unix).
    pub lock_on: ResMut<'w, kuluu_render::LockOn>,

    pub auto_attack: ResMut<'w, crate::view_native::auto_target::AutoAttack>,

    pub status_profile_open: ResMut<'w, kuluu_render::hud::status_panel::StatusProfileOpen>,

    pub sort_options: ResMut<'w, kuluu_render::hud::item_detail::SortOptions>,

    pub item_menu_focus: ResMut<'w, kuluu_render::hud::item_detail::ItemMenuFocus>,

    pub item_screen_container: ResMut<'w, kuluu_render::hud::item_screen::ItemScreenContainer>,

    pub item_viewport: ResMut<'w, kuluu_render::hud::item_screen::ItemListViewport>,

    pub check_target: ResMut<'w, kuluu_render::hud::check_view::CheckTarget>,

    pub bazaar_state: ResMut<'w, kuluu_render::hud::bazaar_view::BazaarScreenState>,

    pub trade_state: ResMut<'w, kuluu_render::hud::trade::TradeState>,

    pub trade_intent: MessageWriter<'w, kuluu_render::hud::trade::TradeIntent>,

    pub delivery_state: ResMut<'w, kuluu_render::hud::delivery::DeliveryScreenState>,

    pub delivery_inv: Res<'w, kuluu_render::hud::delivery::DeliveryInventory>,

    pub auction_state: ResMut<'w, kuluu_render::hud::auction::AuctionScreenState>,

    pub auction_inv: Res<'w, kuluu_render::hud::auction::AuctionSellInventory>,

    pub shop_state: ResMut<'w, kuluu_render::hud::shop::ShopScreenState>,

    pub fishing_spot: Res<'w, kuluu_render::fishing_spot::FishingSpot>,

    pub active_chat_tab: ResMut<'w, ActiveChatTab>,
    pub battle_scroll: ResMut<'w, kuluu_render::hud::chat_panel::BattleScroll>,
    pub debug_scroll: ResMut<'w, kuluu_render::hud::chat_panel::DebugScroll>,

    pub chat_history: ResMut<'w, ChatHistory>,

    pub map_screen_state: ResMut<'w, kuluu_render::hud::map_screen::MapScreenState>,

    pub map_markers: ResMut<'w, kuluu_render::hud::map_screen::MapMarkers>,

    pub map_view: Res<'w, kuluu_render::hud::map_screen::MapView>,

    pub change_map_catalog: Res<'w, kuluu_render::hud::map_screen::ChangeMapCatalog>,

    pub death_prompt: ResMut<'w, kuluu_render::hud::death_prompt::DeathPromptSelection>,

    pub(crate) dat_root: Res<'w, super::DatRootRes>,

    /// Absent when no config dir resolved, which makes `//overlay` read-only.
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

/// The navmesh overlay's visibility latch and cached mesh as one parameter:
/// bevy_ecs's `impl_system_function` tops out at 16 parameters and
/// `text_input_system` sits at that cap on unix (its `AgentPaused` parameter
/// is unix-only), so the two overlay resources ride in together.
#[derive(SystemParam)]
pub struct NavmeshOverlay<'w> {
    pub visible: ResMut<'w, super::navmesh_overlay::NavmeshOverlayVisible>,
    pub state: Res<'w, super::navmesh_overlay::NavmeshState>,
}

/// The two optional session-gate resources ride in together: bevy_ecs's
/// `impl_system_function` tops out at 16 parameters and `text_input_system`
/// sits at that cap on unix, so the pair cannot both take a slot.
#[derive(SystemParam)]
pub(crate) struct SessionGates<'w> {
    #[cfg(unix)]
    pub agent_paused: Option<Res<'w, super::AgentPaused>>,
    pub session_event_tx: Option<Res<'w, super::SessionEventTx>>,
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
use crate::view_native::input::CommandTx;
use crate::view_native::slash_commands::{
    parse_slash, system_chat_line, KeybindUpdate, SlashOutcome, SubAreaOp,
};
#[cfg(unix)]
use kuluu_session::state::AgentEvent;
use kuluu_session::state::{ActionKind, AgentCommand, CheckKind, ReqLogoutKind};

/// The chat/input system: slash commands, menus, and the sub-target picker.
///
/// A Switch Target confirm holds the picker up until the server's 0x058
/// (`vendor/server/src/map/packets/s2c/0x058_assist.cpp`) commits the
/// candidate into the main target (apply_server_retarget_system runs before
/// this one); the frame tracks the candidate meanwhile, so the swap lands on
/// the server's word. The lapse covers the server answering with a rejection
/// line instead (its not-engaged fall-through). If the main target dies while
/// the picker is up (auto_clear dropped it, which runs before this system),
/// the switch is moot: close the picker; the weapon sheathes on its own since
/// the pose pass gates the weapon on an active target. A switch already in
/// flight keeps waiting for its 0x058 even if the old target dies first (the
/// commit lands the new one); only a picker with no pending switch closes on
/// the main target going away.
pub(crate) fn text_input_system(
    mut events: KeyEventStreams,
    cmd_tx: Res<CommandTx>,
    mut bindings: ResMut<Bindings>,
    mut keybinds_state: ResMut<KeybindsStateRes>,
    mut mode: ResMut<InputMode>,
    mut target: ResMut<Target>,
    mut scene_state: ResMut<SceneState>,
    mut exit: MessageWriter<AppExit>,
    mut navmesh: NavmeshOverlay,

    session_gates: SessionGates,

    mut slash_writers: SlashWriters,

    mut draw_distance: ResMut<kuluu_render::dat_mzb::DrawDistance>,

    mut chat_scroll: ResMut<ChatScroll>,

    dynamic_menu: Res<kuluu_render::hud::menu::DynamicMenu>,

    cutscene_mode: Res<kuluu_render::cutscene::CutsceneMode>,
) {
    let entities = scene_state.snapshot.entities.clone();
    let self_pos = scene_state.snapshot.self_pos.pos;
    let current_target = target.id;
    let engaged = matches!(
        scene_state.snapshot.current_goal,
        Some(kuluu_snapshot::ReactorGoal::Engaged { .. })
    );

    let target_changed = target.is_changed();

    if let InputMode::SubTarget(st) = &mut *mode {
        let main_target_gone = matches!(
            st.action,
            kuluu_render::input_mode::SubTargetAction::PickSub
        ) && st.pending_switch.is_none()
            && target.id.is_none();
        let close = main_target_gone
            || st.pending_switch.is_some_and(|sent| {
                target.id == Some(sent)
                    || st
                        .pending_since
                        .is_some_and(|since| since.elapsed() >= SWITCH_ANSWER_TIMEOUT)
            });
        if close {
            st.pending_switch = None;
            st.pending_since = None;
            *mode = InputMode::World;
        }
    }

    let pad_synth: Vec<KeyboardInput> = events.pad.read().map(|e| e.0.clone()).collect();
    for ev in events.keyboard.read().chain(pad_synth.iter()) {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        // CS input lock: while a VM-driven event frame is up, retail's DEFCAMERA has taken the
        // camera and disabled menu drawing (research/XiEvents/OpCodes/0x0046.md), so every key
        // routes through the dialog handler — Enter advances the event even with the map open in
        // Menu(Map) mode, and ESC respects cancel_armed instead of closing whatever window is up.
        if cutscene_mode.active && scene_state.snapshot.dialog.is_some() {
            let next = match &mut *mode {
                InputMode::Dialog(cursor) => handle_dialog_key(
                    &ev.logical_key,
                    &bindings,
                    cursor,
                    &mut scene_state,
                    &cmd_tx.0,
                    &mut slash_writers.item_screen_container,
                ),
                _ => {
                    let mut cursor = DialogCursor::default();
                    handle_dialog_key(
                        &ev.logical_key,
                        &bindings,
                        &mut cursor,
                        &mut scene_state,
                        &cmd_tx.0,
                        &mut slash_writers.item_screen_container,
                    )
                }
            };
            if let Some(next) = next {
                *mode = next;
            }
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
                if bindings.matches_logical(Action::SelectActiveWindow, &ev.logical_key) {
                    if slash_writers.graphics.chat_layout
                        != kuluu_render::graphics_settings::ChatLayout::Tabbed
                    {
                        slash_writers.active_chat_tab.0 =
                            kuluu_render::hud::chat_panel::ChatKind::Social;
                    }
                    *mode = InputMode::PassiveCursor(
                        kuluu_render::input_mode::PassiveCursorState::fresh_chat(),
                    );
                    continue;
                }
                let self_char_id = scene_state.snapshot.self_char_id;
                let usable_items = kuluu_render::hud::menu::any_usable_item(&scene_state.snapshot);
                let can_fish = slash_writers.fishing_spot.0.is_ready();
                let mounted = scene_state.snapshot.self_mount.is_some();
                if let Some(next) = handle_world_key(
                    &ev.logical_key,
                    &bindings,
                    current_target,
                    &entities,
                    self_pos,
                    self_char_id,
                    target_changed,
                    engaged,
                    usable_items,
                    can_fish,
                    mounted,
                    &cmd_tx.0,
                    &mut scene_state,
                    &mut slash_writers.check_target,
                    &mut slash_writers.trade_state,
                    &mut slash_writers.lock_on,
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
                    &mut navmesh.visible,
                    &navmesh.state,
                    &mut bindings,
                    &mut keybinds_state,
                    #[cfg(unix)]
                    session_gates.agent_paused.as_deref(),
                    session_gates.session_event_tx.as_deref(),
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
                    &mut slash_writers.item_viewport,
                    &dynamic_menu,
                    current_target,
                    self_pos,
                    &mut slash_writers.map_screen_state,
                    slash_writers.map_markers.reborrow(),
                    &slash_writers.map_view,
                    &slash_writers.minimap_state,
                    &slash_writers.change_map_catalog,
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
                use kuluu_render::hud::chat_panel::ChatKind;
                let scroll_rows = match slash_writers.active_chat_tab.0 {
                    ChatKind::Social => &mut chat_scroll.rows,
                    ChatKind::Battle => &mut slash_writers.battle_scroll.rows,
                    ChatKind::Debug => &mut slash_writers.debug_scroll.rows,
                };
                if let Some(next) = handle_passive_cursor_key(
                    &ev.logical_key,
                    &bindings,
                    state,
                    scroll_rows,
                    &mut slash_writers.active_chat_tab,
                    slash_writers.graphics.chat_layout,
                    slash_writers.graphics.debug_chat,
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
                    &mut slash_writers.lock_on,
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
            InputMode::Shop => {
                if let Some(next) = handle_shop_key(
                    &ev.logical_key,
                    &bindings,
                    &mut slash_writers.shop_state,
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
        (InputMode::Dialog(c), false) => {
            cursors.closed(c.cursor);
            *mode = InputMode::World;
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
/// on when Esc backs out (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md).
#[derive(Default)]
pub struct DialogCursors {
    open: Option<u64>,
    seen: std::collections::HashMap<u64, u32>,
}

/// Rows remembered across closes before the map is dropped. The memory is a
/// convenience; a session's server-driven event frames are unbounded and this
/// map is a `Local` that outlives every one of them.
const CURSOR_MEMORY_FRAMES: usize = 64;

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

    /// `seen` outlives the close: retail reopens the Mog Menu on the row it was
    /// left on (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md,
    /// "How the menu opens").
    fn closed(&mut self, cursor: u32) {
        if let Some(left) = self.open.take() {
            if self.seen.len() >= CURSOR_MEMORY_FRAMES && !self.seen.contains_key(&left) {
                self.seen.clear();
            }
            self.seen.insert(left, cursor);
        }
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
    use kuluu_render::hud::trade::{self, TradeFocus};

    if let Some(spinner) = trade_state.selector.as_mut() {
        if bindings.matches_logical(Action::NavConfirm, key) {
            trade::gil_confirm(trade_state);
        } else if bindings.matches_logical(Action::NavCancel, key) {
            trade_state.selector = None;
        } else {
            spinner_nav(spinner, key, bindings);
        }
        return None;
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
                    &slash_writers.command_surface,
                    entities,
                    self_pos,
                    current_target,
                    scene_state.snapshot.zone_id,
                    scene_state.snapshot.self_char_id,
                    &scene_state.snapshot.party,
                    fishing_gate,
                    match scene_state.snapshot.current_goal {
                        Some(kuluu_snapshot::ReactorGoal::Engaged { target_id, .. }) => {
                            Some(target_id)
                        }
                        _ => None,
                    },
                    scene_state.snapshot.self_pet_targid,
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
                    SlashOutcome::OpenSubTarget { action, narrow } => open_sub_target_narrowed(
                        *action,
                        *narrow,
                        current_target,
                        scene_state,
                        InputMode::World,
                    ),
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

/// Menu actions that take retail's sub-target confirm step before firing:
/// every spell, ability, weapon skill, ranged attack and usable item, SELF-only
/// ones included (the cursor lands on the player and waits for Enter). Move,
/// equip, emote and the mount/dig toggles act without a target and dispatch
/// immediately. (.agents/skills/retail-observe/references/2026-09-21-action-confirm-and-locks.md,
/// "Menu actions always confirm through the sub-target cursor".)
fn sub_target_action_for(
    action: kuluu_render::hud::menu::DynamicMenuAction,
) -> Option<kuluu_render::input_mode::SubTargetAction> {
    use kuluu_render::hud::menu::DynamicMenuAction as A;
    use kuluu_render::input_mode::SubTargetAction as S;
    match action {
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
        A::DropItem { .. } => None,
        A::EquipItem { .. } => None,
        A::KeyItem { .. } => None,
        A::Emote { .. } => None,
    }
}

/// Inverse of `sub_target_action_for`, used to fire the pending action once
/// the sub-target cursor is confirmed. Job vs pet ability collapses to
/// JobAbility; their dispatch is identical. PickSub does not fire an action:
/// handle_sub_target_key stores the candidate in the sub slot before this is
/// ever called.
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
        S::PickSub => unreachable!("PickSub is resolved before dispatch"),
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
    let self_pet = snap.self_pet_targid;
    let self_pos = snap.self_pos.pos;
    // A pet joins the enemy set only when its allegiance differs from ours:
    // the server's TARGET_ENEMY check is the allegiance inequality
    // (vendor/server/src/map/entities/battle_entity.cpp
    // CBattleEntity::ValidTarget) and a pet inherits its master's allegiance
    // (vendor/server/src/map/utils/petutils.cpp). The 0x00A self sync can OR
    // in a display belligerence bit (vendor/server/src/map/packets/
    // char_status.cpp), so compare the low three bits.
    let self_allegiance = self_id
        .and_then(|id| snap.entities.iter().find(|e| e.id == id))
        .map(|e| e.char_flags.allegiance & 7);
    let self_party = snap
        .party
        .iter()
        .find(|m| Some(m.id) == self_id)
        .map_or(0, |m| m.party_no);
    snap.entities
        .iter()
        .map(|e| {
            let dx = e.pos.x - self_pos.x;
            let dy = e.pos.y - self_pos.y;
            let dz = e.pos.z - self_pos.z;
            let member = snap.party.iter().find(|m| m.id == e.id);
            let is_party = member.is_some_and(|m| m.party_no == self_party);
            kuluu_render::sub_target::SubTargetEntity {
                id: e.id,
                is_self: Some(e.id) == self_id,
                is_pc: matches!(e.kind, EntityKind::Pc),
                is_party,
                is_alliance: member.is_some(),
                is_enemy: matches!(e.kind, EntityKind::Mob)
                    || (matches!(e.kind, EntityKind::Pet)
                        && self_allegiance.is_some_and(|a| e.char_flags.allegiance & 7 != a)),
                is_npc: matches!(e.kind, EntityKind::Npc),
                is_own_pet: self_pet.is_some_and(|t| e.act_index == t),
                is_dead: e.hp_pct == Some(0),
                dist_sq: dx * dx + dy * dy + dz * dz,
            }
        })
        .collect()
}

/// Open the retail sub-target confirm step for `action`. Returns None (stay
/// in the current mode) when nothing in range qualifies, echoing retail's
/// refusal line. "Switch Target" picks a *different* mob: parking the cursor
/// on the main target would make the first confirm a no-op, so the picker
/// starts on the nearest valid candidate other than it.
/// True when `current_target` satisfies the action's validTarget flags. The
/// target-action menu still dispatches directly on a valid target; the main
/// menus always confirm through the cursor and do not call this.
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
    use kuluu_render::input_mode::SubTargetAction;
    use kuluu_render::sub_target;
    let flags = sub_target::action_flags(action);
    let ents = gather_sub_target_entities(scene_state);
    let parked = if matches!(action, SubTargetAction::PickSub) {
        None
    } else {
        current_target
    };
    let candidate = if matches!(action, SubTargetAction::PickSub) {
        sub_target::initial_candidate(flags, None, &ents)
            .filter(|id| Some(*id) != current_target)
            .or_else(|| {
                ents.iter()
                    .filter(|e| e.id != current_target.unwrap_or(0))
                    .filter(|e| sub_target::entity_valid(flags, e))
                    .min_by(|a, b| a.dist_sq.total_cmp(&b.dist_sq))
                    .map(|e| e.id)
            })
    } else {
        sub_target::initial_candidate(flags, parked, &ents)
    };
    let Some(candidate) = candidate else {
        push_system_chat_line(scene_state, "Unable to see any qualified targets.".into());
        return None;
    };
    let mut st = kuluu_render::input_mode::SubTargetState::open(action, flags.0, return_to);
    st.candidate = Some(candidate);
    Some(InputMode::SubTarget(st))
}

fn gather_filtered_sub_targets(
    scene_state: &SceneState,
    filter: Option<u16>,
) -> Vec<kuluu_render::sub_target::SubTargetEntity> {
    let mut entities = gather_sub_target_entities(scene_state);
    if let Some(filter) = filter {
        entities.retain(|entity| {
            let mut membership = *entity;
            membership.is_dead = false;
            kuluu_render::sub_target::entity_valid(
                ffxi_vocab::valid_target::TargetFlags(filter),
                &membership,
            )
        });
    }
    entities
}

fn open_sub_target_narrowed(
    action: kuluu_render::input_mode::SubTargetAction,
    narrow: Option<ffxi_vocab::valid_target::TargetFlags>,
    current_target: Option<u32>,
    scene_state: &mut SceneState,
    return_to: InputMode,
) -> Option<InputMode> {
    let Some(narrow) = narrow else {
        return open_sub_target(action, current_target, scene_state, return_to);
    };
    use kuluu_render::sub_target;
    let flags = sub_target::action_flags(action);
    let ents = gather_filtered_sub_targets(scene_state, Some(narrow.0));
    let Some(candidate) = sub_target::initial_candidate(flags, current_target, &ents) else {
        push_system_chat_line(scene_state, "Unable to see any qualified targets.".into());
        return None;
    };
    let mut st = kuluu_render::input_mode::SubTargetState::open(action, flags.0, return_to);
    st.candidate = Some(candidate);
    st.candidate_filter = Some(narrow.0);
    Some(InputMode::SubTarget(st))
}

/// How long a Switch Target confirm waits for the server's 0x058
/// (`vendor/server/src/map/packets/s2c/0x058_assist.cpp`) before the picker
/// closes on its own; the answer lands within one AI tick, so this only
/// covers the server refusing with a rejection line instead.
const SWITCH_ANSWER_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// Retail sub-target cursor keys: Tab/arrows cycle valid candidates in
/// distance order, Enter fires the pending action at the candidate, Esc
/// returns to the originating menu with its cursor preserved. A Switch Target
/// confirm is gated by the local engage pre-checks, the only range/claim gate
/// on that path: a candidate the server would refuse at the next swing
/// (36/12 + disengage) does not leave the client — the server keeps swinging
/// the current target and the cursor stays up. Re-picking cancels the
/// in-flight switch: the sent 0x058
/// (vendor/server/src/map/packets/s2c/0x058_assist.cpp) may still land the
/// main target, but the picker stays up for the new choice.
fn handle_sub_target_key(
    key: &Key,
    bindings: &Bindings,
    state: &mut kuluu_render::input_mode::SubTargetState,
    scene_state: &mut SceneState,
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    use ffxi_vocab::valid_target::TargetFlags;
    use kuluu_render::input_mode::SubTargetAction;
    use kuluu_render::sub_target;

    let flags = TargetFlags(state.flags);
    let ents = gather_filtered_sub_targets(scene_state, state.candidate_filter);

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
        state.pending_switch = None;
        state.pending_since = None;
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
        if matches!(state.action, SubTargetAction::PickSub) {
            // "Switch Target" asks the server to move the battle target: c2s
            // 0x01A ChangeTarget, whose engaged branch is a bare
            // setBattleTarget with no entry validation and no swing-delay
            // gate (vendor/server/src/map/ai/ai_container.cpp
            // CAIContainer::Internal_ChangeTarget). The server's 0x058 echo —
            // one AI tick later — lands it in the main target slot
            // (apply_server_retarget_system), so the picker holds until then.
            let Some(ent) = entities.iter().find(|e| e.id == id) else {
                push_system_chat_line(scene_state, "Unable to see any qualified targets.".into());
                return None;
            };
            if let Some(line) = crate::view_native::engage::rejection_line(
                ent,
                scene_state.snapshot.self_pos.pos,
                scene_state.snapshot.self_char_id,
                &scene_state.snapshot.party,
            ) {
                push_system_chat_line(scene_state, line);
                return None;
            }
            if let Err(err) = cmd_tx.try_send(AgentCommand::Action {
                target_id: id,
                target_index: ent.act_index,
                kind: ActionKind::ChangeTarget,
            }) {
                push_system_chat_line(
                    scene_state,
                    format!("[menu] Switch Target dispatch dropped: {err}"),
                );
                return None;
            }
            state.pending_switch = Some(id);
            state.pending_since = Some(std::time::Instant::now());
            return None;
        }
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

/// Fires a confirmed dynamic-menu action. OpenItemAction and DropItem are
/// pushed as submenus by confirm_menu_at_cursor and do not dispatch here.
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
        A::OpenItemAction { .. } | A::DropItem { .. } => return,
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
                // container: vendor/server/src/map/utils/charutils.cpp EquipItem
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
        // The player answered the frame manually: the auto-enter clock holds
        // its fire while this timestamp is inside its guard window, so the
        // session's round-trip can't double-advance (auto_enter.rs).
        scene_state.last_manual_dialog_advance = Some(std::time::Instant::now());
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
    mut lock_on: ResMut<kuluu_render::LockOn>,
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
            let entries = kuluu_render::hud::overlay::RETAIL.resolve_target_actions(&state.ctx);
            if state.dismount_confirm {
                // The pane shows Yes/No, not the rows: slot 0 is Yes.
                if let Some(next) = answer_dismount_confirm(
                    state,
                    &entries,
                    ev.slot == 0,
                    &mut scene_state,
                    &entities,
                    &cmd_tx.0,
                ) {
                    *mode = next;
                }
                continue;
            }
            state.cursor = ev.slot;

            if let Some(next) = confirm_target_action_at_cursor(
                state,
                &entries,
                &mut scene_state,
                current_target,
                &entities,
                &cmd_tx.0,
                &mut check_target,
                &mut trade_state,
                &mut lock_on,
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

/// Dialog/cutscene key handling. ESC reconciliation goes through the session
/// snapshot (clearing locally flickers multi-frame events): the session
/// decides whether this pops a client-local menu level or ends the
/// interaction, because only it knows the depth.
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
        // Retail's 0x42 (research/XiEvents/OpCodes/0x0042.md) disarms
        // ESC-cancel in the prologue of cutscenes that lock you in (event 503
        // runs it as its second opcode); while disarmed, ESC is a no-op — no
        // EVENT_END goes out and the event keeps running.
        if scene_state
            .snapshot
            .dialog
            .as_ref()
            .is_some_and(|d| !d.cancel_armed)
        {
            return None;
        }
        let _ = cmd_tx.try_send(AgentCommand::EndEventBack);
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
    scroll_rows: &mut usize,
    active_chat_tab: &mut ActiveChatTab,
    layout: kuluu_render::graphics_settings::ChatLayout,
    debug_chat: bool,
    scene_state: &SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    use kuluu_render::input_mode::{PassiveCursorFocus, PassiveCursorState};

    let icons = &scene_state.snapshot.status_icons;

    if bindings.matches_logical(Action::SelectActiveWindow, key) {
        if state.focus == PassiveCursorFocus::Chat
            && kuluu_render::hud::chat_panel::advance_split_focus(
                &mut active_chat_tab.0,
                layout,
                debug_chat,
            )
        {
            state.chat_expanded = false;
            return None;
        }
        return Some(match state.focus {
            PassiveCursorFocus::Chat if !icons.is_empty() => {
                InputMode::PassiveCursor(PassiveCursorState::fresh_status())
            }
            _ => InputMode::World,
        });
    }

    match state.focus {
        PassiveCursorFocus::Chat => {
            let max_back = kuluu_render::snapshot::rendered_chat(scene_state)
                .iter()
                .filter(|line| {
                    active_chat_tab.0.accepts_in_layout(line.channel, layout)
                        && kuluu_render::snapshot::chat_line_visible(line.channel, debug_chat)
                })
                .count();
            if bindings.matches_logical(Action::NavUp, key) {
                if *scroll_rows + 1 < max_back {
                    *scroll_rows += 1;
                }
                return None;
            }
            if bindings.matches_logical(Action::NavDown, key) {
                *scroll_rows = scroll_rows.saturating_sub(1);
                return None;
            }
            if bindings.matches_logical(Action::PageUp, key) {
                let next = scroll_rows.saturating_add(CHAT_SCROLL_PAGE_ROWS);
                *scroll_rows = next.min(max_back.saturating_sub(1));
                return None;
            }
            if bindings.matches_logical(Action::PageDown, key) {
                *scroll_rows = scroll_rows.saturating_sub(CHAT_SCROLL_PAGE_ROWS);
                return None;
            }
            // Left/Right cycle which chat tab the focused log shows.
            if layout != kuluu_render::graphics_settings::ChatLayout::Unified
                && bindings.matches_logical(Action::NavLeft, key)
            {
                active_chat_tab.0 = active_chat_tab.0.step(false, debug_chat);
                return None;
            }
            if layout != kuluu_render::graphics_settings::ChatLayout::Unified
                && bindings.matches_logical(Action::NavRight, key)
            {
                active_chat_tab.0 = active_chat_tab.0.step(true, debug_chat);
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
mod chat_window_tests {
    use super::*;
    use kuluu_render::graphics_settings::ChatLayout;
    use kuluu_render::hud::chat_panel::ChatKind;
    use kuluu_render::input_mode::PassiveCursorState;

    #[test]
    fn unified_chat_keeps_focus_on_the_single_log() {
        let bindings = kuluu_render::keybinds::presets::compact1();
        let mut state = PassiveCursorState::fresh_chat();
        let mut active = ActiveChatTab(ChatKind::Social);
        let scene = SceneState::default();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let mut rows = 0;
        for key in [Key::ArrowLeft, Key::ArrowRight] {
            handle_passive_cursor_key(
                &key,
                &bindings,
                &mut state,
                &mut rows,
                &mut active,
                ChatLayout::Unified,
                true,
                &scene,
                &tx,
            );
            assert_eq!(active.0, ChatKind::Social);
        }
        assert!(matches!(
            handle_passive_cursor_key(
                &Key::Character("f".into()),
                &bindings,
                &mut state,
                &mut rows,
                &mut active,
                ChatLayout::Unified,
                true,
                &scene,
                &tx,
            ),
            Some(InputMode::World)
        ));
    }

    #[test]
    fn compact_f_selects_second_split_log_before_releasing_focus() {
        let bindings = kuluu_render::keybinds::presets::compact1();
        let mut state = PassiveCursorState::fresh_chat();
        let mut active = ActiveChatTab(ChatKind::Social);
        let scene = SceneState::default();
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let mut rows = 0;
        let key = Key::Character("f".into());
        for layout in [ChatLayout::Vertical, ChatLayout::SideBySide] {
            active.0 = ChatKind::Social;
            assert!(handle_passive_cursor_key(
                &key,
                &bindings,
                &mut state,
                &mut rows,
                &mut active,
                layout,
                false,
                &scene,
                &tx
            )
            .is_none());
            assert_eq!(active.0, ChatKind::Battle);
            assert!(matches!(
                handle_passive_cursor_key(
                    &key,
                    &bindings,
                    &mut state,
                    &mut rows,
                    &mut active,
                    layout,
                    false,
                    &scene,
                    &tx
                ),
                Some(InputMode::World)
            ));
        }
    }

    #[test]
    fn backscroll_is_bounded_by_the_selected_log() {
        let bindings = kuluu_render::keybinds::presets::compact1();
        let mut state = PassiveCursorState::fresh_chat();
        let mut active = ActiveChatTab(ChatKind::Battle);
        let mut scene = SceneState::default();
        for channel in [
            kuluu_snapshot::ChatChannel::Say,
            kuluu_snapshot::ChatChannel::System,
            kuluu_snapshot::ChatChannel::Battle,
        ] {
            let mut line = kuluu_render::snapshot::system_chat_line("message".into());
            line.channel = channel;
            scene.snapshot.chat.push(line);
        }
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let mut rows = 0;
        for _ in 0..3 {
            handle_passive_cursor_key(
                &Key::ArrowUp,
                &bindings,
                &mut state,
                &mut rows,
                &mut active,
                ChatLayout::SideBySide,
                false,
                &scene,
                &tx,
            );
        }
        assert_eq!(rows, 1);
        handle_passive_cursor_key(
            &Key::ArrowDown,
            &bindings,
            &mut state,
            &mut rows,
            &mut active,
            ChatLayout::SideBySide,
            false,
            &scene,
            &tx,
        );
        assert_eq!(rows, 0);
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

    /// Retail reopens a menu on the row it was left on, including across a
    /// close (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md,
    /// "How the menu opens").
    #[test]
    fn the_row_a_menu_was_left_on_survives_a_close() {
        let mut cursors = DialogCursors::default();
        cursors.switch(Some(MOG_ROOT), 0, NO_GRID);
        cursors.switch(Some(DELIVERY_SUBMENU), 3, NO_GRID);
        cursors.closed(1);
        assert_eq!(
            cursors.switch(Some(DELIVERY_SUBMENU), 0, NO_GRID),
            Some(1),
            "the row the panel was closed on"
        );
        cursors.switch(Some(MOG_ROOT), 0, NO_GRID);
        assert_eq!(
            cursors.switch(Some(MOG_ROOT), 0, NO_GRID),
            None,
            "already showing"
        );
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
            monstrosity: false,
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

#[cfg(test)]
mod dialog_esc_gate_tests {
    use super::*;
    use kuluu_snapshot::DialogState;

    /// Drains a tokio mpsc receiver without a runtime (try_recv only).
    pub(super) fn drain(
        cmd_rx: &mut tokio::sync::mpsc::Receiver<AgentCommand>,
    ) -> Vec<AgentCommand> {
        let mut sent = Vec::new();
        while let Ok(msg) = cmd_rx.try_recv() {
            sent.push(msg);
        }
        sent
    }

    /// Event 503's master block runs 0x42 (research/XiEvents/OpCodes/0x0042.md)
    /// as its second opcode: while the VM reports cancel_armed=false, ESC must
    /// not send any command (retail locks you in; no EVENT_END goes out at all).
    #[test]
    fn esc_is_a_noop_while_the_vm_has_disarmed_cancel() {
        let bindings = Bindings::default();
        let mut cursor = DialogCursor::default();
        let mut scene_state = SceneState::default();
        let dialog = DialogState {
            cancel_armed: false,
            ..Default::default()
        };
        scene_state.snapshot.dialog = Some(dialog);

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();

        assert!(handle_dialog_key(
            &Key::Escape,
            &bindings,
            &mut cursor,
            &mut scene_state,
            &cmd_tx,
            &mut item_bag
        )
        .is_none());

        let sent = drain(&mut cmd_rx);
        assert!(
            sent.is_empty(),
            "ESC must not send any command while cancel is disarmed, got {sent:?}"
        );
    }

    /// A plain conversation (cancel_armed=true) still cancels with EndEvent.
    #[test]
    fn esc_sends_end_event_while_cancel_is_armed() {
        let bindings = Bindings::default();
        let mut cursor = DialogCursor::default();
        let mut scene_state = SceneState::default();
        let dialog = DialogState {
            cancel_armed: true,
            ..Default::default()
        };
        scene_state.snapshot.dialog = Some(dialog);

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();

        assert!(handle_dialog_key(
            &Key::Escape,
            &bindings,
            &mut cursor,
            &mut scene_state,
            &cmd_tx,
            &mut item_bag
        )
        .is_none());

        let sent = drain(&mut cmd_rx);
        assert_eq!(sent, vec![AgentCommand::EndEventBack]);
    }
}

/// The CS input lock at the system level: while a VM-driven event frame is up, keys route
/// through the dialog handler even when InputMode says otherwise — Enter advances the event
/// with the map open in Menu(Map) mode (event 503's beat), and ESC respects cancel_armed
/// instead of closing whatever window is up.
#[cfg(test)]
mod cs_input_lock_tests {
    use super::*;
    use bevy::input::keyboard::KeyCode;
    use kuluu_snapshot::DialogState;

    /// The full resource set text_input_system's parameters fetch, so the gate
    /// can be driven on a bare app exactly like cutscene.rs's tests do.
    /// Message storage covers every reader/writer the system carries. The
    /// shutdown-counter feature adds a slash writer for LogoutRequested; the
    /// bare app registers it or the gate's fetch panics. CutsceneMode is
    /// initialized by the plugin in production; the gate reads it
    /// unconditionally, so the bare app carries a default.
    pub(super) fn gate_app(cmd_tx: tokio::sync::mpsc::Sender<AgentCommand>) -> App {
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .add_message::<crate::view_native::gamepad_input::PadKeyEvent>()
            .add_message::<AppExit>()
            .add_message::<LoadMmbRequest>()
            .add_message::<LoadMzbRequest>()
            .add_message::<kuluu_render::sub_area_activation::SetSubArea>()
            .add_message::<DebugHeightsRequest>()
            .add_message::<kuluu_render::audio::SfxEvent>()
            .add_message::<crate::view_native::screenshot::ScreenshotRequest>()
            .add_message::<kuluu_render::hud::trade::TradeIntent>();
        #[cfg(feature = "enhanced-shutdown-counter")]
        app.add_message::<kuluu_render::hud::logout_countdown::LogoutRequested>();
        app.insert_resource(CommandTx(cmd_tx));
        app.insert_resource(Bindings::default());
        app.insert_resource(KeybindsStateRes {
            store: crate::keybinds_store::KeybindsStore::new(
                std::env::temp_dir().join("kuluu-cs-gate-tests-unused.json"),
            ),
            persisted: Default::default(),
        });
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Map);
        app.insert_resource(InputMode::Menu(stack));
        app.insert_resource(Target::default());
        app.insert_resource(kuluu_render::LockOn::default());
        app.insert_resource(crate::view_native::auto_target::AutoAttack::default());
        app.insert_resource(SceneState::default());
        app.insert_resource(kuluu_render::cutscene::CutsceneMode::default());
        app.insert_resource(crate::view_native::navmesh_overlay::NavmeshOverlayVisible::default());
        app.insert_resource(crate::view_native::navmesh_overlay::NavmeshState::default());
        app.insert_resource(bevy_framepace::FramepaceSettings::default());
        app.insert_resource(CaptureMode::default());
        app.insert_resource(kuluu_render::EventLog::default());
        app.insert_resource(kuluu_render::GraphicsSettings::default());
        app.insert_resource(kuluu_render::hud::HudVerbosity::default());
        app.insert_resource(kuluu_render::hud::HudPanels::default());
        app.insert_resource(kuluu_render::hud::network_status::NetStatusVisible::default());
        app.insert_resource(kuluu_render::vana_time::VanaClock::default());
        app.insert_resource(kuluu_render::hud::vana_clock::VanaClockVisible::default());
        app.insert_resource(kuluu_render::minimap::MinimapMode::default());
        app.insert_resource(kuluu_render::minimap::MinimapVisible::default());
        app.insert_resource(kuluu_render::minimap::topdown::TopdownCullPolicy::default());
        app.insert_resource(kuluu_render::audio::AudioMuteState::default());
        app.insert_resource(kuluu_render::minimap::MinimapZoom::default());
        app.insert_resource(kuluu_render::minimap::MinimapView::default());
        app.insert_resource(kuluu_render::minimap::MinimapState::default());
        app.insert_resource(kuluu_render::combat_stance::RestStance::default());
        app.insert_resource(kuluu_render::hud::status_panel::StatusProfileOpen::default());
        app.insert_resource(kuluu_render::hud::item_detail::SortOptions::default());
        app.insert_resource(kuluu_render::hud::item_detail::ItemMenuFocus::default());
        app.insert_resource(kuluu_render::hud::item_screen::ItemScreenContainer::default());
        app.insert_resource(kuluu_render::hud::item_screen::ItemListViewport::default());
        app.insert_resource(kuluu_render::hud::check_view::CheckTarget::default());
        app.insert_resource(kuluu_render::hud::bazaar_view::BazaarScreenState::default());
        app.insert_resource(kuluu_render::hud::trade::TradeState::default());
        app.insert_resource(kuluu_render::hud::shop::ShopScreenState::default());
        app.insert_resource(kuluu_render::hud::delivery::DeliveryScreenState::default());
        app.insert_resource(kuluu_render::hud::delivery::DeliveryInventory::default());
        app.insert_resource(kuluu_render::hud::auction::AuctionScreenState::default());
        app.insert_resource(kuluu_render::hud::auction::AuctionSellInventory::default());
        app.insert_resource(crate::view_native::command_surface::CommandSurface::default());
        app.insert_resource(kuluu_render::fishing_spot::FishingSpot::default());
        app.insert_resource(ActiveChatTab::default());
        app.insert_resource(ChatHistory::default());
        app.insert_resource(kuluu_render::hud::map_screen::MapScreenState::default());
        app.insert_resource(kuluu_render::hud::map_screen::MapMarkers::default());
        app.insert_resource(kuluu_render::hud::map_screen::MapView::default());
        app.insert_resource(kuluu_render::hud::map_screen::ChangeMapCatalog::default());
        app.insert_resource(kuluu_render::hud::death_prompt::DeathPromptSelection::default());
        app.insert_resource(crate::view_native::DatRootRes(None));
        app.insert_resource(kuluu_render::dat_mzb::DrawDistance::default());
        app.insert_resource(ChatScroll::default());
        app.insert_resource(kuluu_render::hud::chat_panel::BattleScroll::default());
        app.insert_resource(kuluu_render::hud::chat_panel::DebugScroll::default());
        app.insert_resource(kuluu_render::hud::menu::DynamicMenu::default());
        app.add_systems(Update, text_input_system);
        app
    }

    fn press(app: &mut App, key: Key) {
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(KeyboardInput {
                key_code: KeyCode::Enter,
                logical_key: key,
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
    }

    /// Event 503's map beat: the coupon line is up with the map open in Menu(Map) mode and the
    /// VM disarmed ESC-cancel. Enter must advance the event (EndEventChoice), not fall through
    /// to the menu handler — without the gate a human playthrough hangs here.
    #[test]
    fn enter_advances_the_event_with_the_map_open() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = gate_app(cmd_tx);
        app.world_mut().resource_mut::<SceneState>().snapshot.dialog = Some(DialogState {
            cancel_armed: false,
            ..Default::default()
        });
        app.insert_resource(kuluu_render::cutscene::CutsceneMode::active_locked());

        press(&mut app, Key::Enter);
        app.update();

        let sent = dialog_esc_gate_tests::drain(&mut cmd_rx);
        assert_eq!(
            sent,
            vec![AgentCommand::EndEventChoice {
                event_id: 0,
                act_index: 0,
                event_num: 0,
                choice: 0
            }],
            "Enter must advance the event, got {sent:?}"
        );
    }

    /// Same state, ESC instead: with cancel disarmed (event 503's second opcode) no command may
    /// go out at all — the map cannot be closed by hand while the frame is up.
    #[test]
    fn esc_cannot_close_the_map_while_disarmed() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = gate_app(cmd_tx);
        app.world_mut().resource_mut::<SceneState>().snapshot.dialog = Some(DialogState {
            cancel_armed: false,
            ..Default::default()
        });
        app.insert_resource(kuluu_render::cutscene::CutsceneMode::active_locked());

        press(&mut app, Key::Escape);
        app.update();

        let sent = dialog_esc_gate_tests::drain(&mut cmd_rx);
        assert!(
            sent.is_empty(),
            "ESC must be a no-op while disarmed, got {sent:?}"
        );
    }

    /// The gate must not overreach: with no cutscene session active (the gate
    /// app's default CutsceneMode) the same Menu(Map) + frame state routes to
    /// the menu handler as before — its map-beat branch cancels the event
    /// unconditionally (no cancel_armed check), which is exactly what the
    /// gated path refuses.
    #[test]
    fn without_a_cutscene_session_the_menu_handler_keeps_the_keys() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = gate_app(cmd_tx);
        app.world_mut().resource_mut::<SceneState>().snapshot.dialog = Some(DialogState {
            cancel_armed: false,
            ..Default::default()
        });
        press(&mut app, Key::Escape);
        app.update();

        let sent = dialog_esc_gate_tests::drain(&mut cmd_rx);
        assert_eq!(
            sent,
            vec![AgentCommand::EndEvent],
            "the map handler's own cancel must run when no CS session is active, got {sent:?}"
        );
    }
}

/// Auto-Enter CS at the system level: with the Debug row on, eligible
/// message frames advance themselves after their read time (the same
/// `EndEventChoice` Enter would send), and the frames the player must answer
/// stay manual.
#[cfg(test)]
mod auto_enter_tests {
    use super::*;
    use kuluu_snapshot::DialogState;

    /// A headless app carrying only auto_enter_cs_system's parameters;
    /// MinimalPlugins supplies the Time resource and its per-update advance.
    fn auto_enter_app(cmd_tx: tokio::sync::mpsc::Sender<AgentCommand>) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(CommandTx(cmd_tx));
        app.insert_resource(kuluu_render::hud::HudPanels::default());
        app.insert_resource(SceneState::default());
        app.add_systems(Update, auto_enter_cs_system);
        app
    }

    fn eligible_frame() -> DialogState {
        DialogState {
            npc_id: 0x010E6001,
            act_index: 7,
            event_para: 230,
            prompt: Some("A line of narration.".into()),
            ..Default::default()
        }
    }

    #[test]
    fn off_sends_nothing() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = auto_enter_app(cmd_tx);
        app.world_mut().resource_mut::<SceneState>().snapshot.dialog = Some(eligible_frame());
        for _ in 0..3 {
            app.update();
        }
        assert!(dialog_esc_gate_tests::drain(&mut cmd_rx).is_empty());
    }

    #[test]
    fn choice_frames_are_never_advanced() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = auto_enter_app(cmd_tx);
        let mut d = eligible_frame();
        d.choices.push("Yes".into());
        app.world_mut().resource_mut::<SceneState>().snapshot.dialog = Some(d);
        app.world_mut()
            .resource_mut::<kuluu_render::hud::HudPanels>()
            .auto_enter_cs = true;
        for _ in 0..3 {
            app.update();
        }
        assert!(dialog_esc_gate_tests::drain(&mut cmd_rx).is_empty());
    }

    #[test]
    fn blacklisted_speakers_are_never_advanced() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = auto_enter_app(cmd_tx);
        let mut d = eligible_frame();
        d.npc_name = Some("Paintbrush of Souls".into());
        app.world_mut().resource_mut::<SceneState>().snapshot.dialog = Some(d);
        app.world_mut()
            .resource_mut::<kuluu_render::hud::HudPanels>()
            .auto_enter_cs = true;
        for _ in 0..3 {
            app.update();
        }
        assert!(dialog_esc_gate_tests::drain(&mut cmd_rx).is_empty());
    }

    /// The read-time floor is 1.5 s of wall clock, so this test runs real
    /// time; the deadline bounds a wedged clock.
    #[test]
    fn eligible_frame_advances_exactly_once_after_its_read_time() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = auto_enter_app(cmd_tx);
        app.world_mut().resource_mut::<SceneState>().snapshot.dialog = Some(eligible_frame());
        app.world_mut()
            .resource_mut::<kuluu_render::hud::HudPanels>()
            .auto_enter_cs = true;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        let mut sent = Vec::new();
        while sent.is_empty() && std::time::Instant::now() < deadline {
            app.update();
            sent = dialog_esc_gate_tests::drain(&mut cmd_rx);
        }
        assert_eq!(
            sent,
            vec![AgentCommand::EndEventChoice {
                event_id: 0x010E6001,
                act_index: 7,
                event_num: 230,
                choice: 0
            }],
            "the eligible frame must advance exactly once, got {sent:?}"
        );
    }

    /// A manual Enter landing just before the clock's fire must hold it: the
    /// snapshot is still on the pre-advance frame, and sending would make the
    /// session dismiss the frame the manual advance just opened. Runs real
    /// time up to the 1.5 s read-time floor plus guard margin.
    #[test]
    fn a_recent_manual_advance_holds_the_fire() {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = auto_enter_app(cmd_tx);
        app.world_mut().resource_mut::<SceneState>().snapshot.dialog = Some(eligible_frame());
        app.world_mut()
            .resource_mut::<kuluu_render::hud::HudPanels>()
            .auto_enter_cs = true;
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(1300) {
            app.update();
        }
        app.world_mut()
            .resource_mut::<SceneState>()
            .last_manual_dialog_advance = Some(std::time::Instant::now());
        while start.elapsed() < std::time::Duration::from_millis(1900) {
            app.update();
        }
        assert!(
            dialog_esc_gate_tests::drain(&mut cmd_rx).is_empty(),
            "the manual-advance guard must hold the fire"
        );
    }
}

/// "Switch Target" (PickSub) parks the sub-target cursor off the main target
/// and confirms into the main target: parking on the main target would make
/// the first Enter a no-op, and confirming re-engages on the chosen mob.
#[cfg(test)]
mod sub_target_pick_tests {
    use super::*;
    use kuluu_render::input_mode::SubTargetAction;
    use kuluu_snapshot::{Entity, EntityKind, PartyMember, Vec3};

    fn ent(id: u32, kind: EntityKind, x: f32) -> Entity {
        Entity {
            id,
            act_index: 0,
            kind,
            name: Some(format!("e{id}")),
            pos: Vec3 { x, y: 0.0, z: 0.0 },
            heading: 0,
            hp_pct: Some(100),
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
            monstrosity: false,
        }
    }

    fn party_member(id: u32) -> PartyMember {
        PartyMember {
            id,
            act_index: 0,
            name: Some(format!("p{id}")),
            hp: 100,
            mp: 100,
            tp: 0,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 0,
            main_job: 1,
            main_job_lv: 1,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            party_no: 0,
            in_mog_house: false,
        }
    }

    const SELF_ID: u32 = 0x0100_0001;
    const MOB_ID: u32 = 0x0200_0001;
    const MOB2_ID: u32 = 0x0200_0002;
    const PARTY_ID: u32 = 0x0100_0002;
    const ENEMY_PET_ID: u32 = 0x0300_0001;
    const OWNED_PET_ID: u32 = 0x0300_0002;
    const PARTY_PET_ID: u32 = 0x0300_0003;

    fn with_allegiance(mut e: Entity, allegiance: u8) -> Entity {
        e.char_flags.allegiance = allegiance;
        e
    }

    // xi::Allegiance values (vendor/server/data/enums/allegiance.yaml): the
    // nation band, so the scene exercises the PVP pet inequality.
    const SELF_NATION: u8 = 2;
    const ENEMY_NATION: u8 = 3;

    /// Self at the origin on the San d'Oria allegiance, the engaged mob 5
    /// yalms out, a second mob 8 yalms out, a party member 3 yalms out, an
    /// enemy pet 6 yalms out, and the owned/party pets 4 yalms out. Pets
    /// carry no claim: the server never sets one on a pet
    /// (vendor/server/src/map/utils/battleutils.cpp ClaimMob).
    fn battle_scene() -> SceneState {
        let mut s = SceneState::default();
        s.snapshot.self_char_id = Some(SELF_ID);
        s.snapshot.entities = vec![
            with_allegiance(ent(SELF_ID, EntityKind::Pc, 0.0), SELF_NATION),
            ent(MOB_ID, EntityKind::Mob, 5.0),
            ent(MOB2_ID, EntityKind::Mob, 8.0),
            ent(PARTY_ID, EntityKind::Pc, 3.0),
            with_allegiance(ent(ENEMY_PET_ID, EntityKind::Pet, 6.0), ENEMY_NATION),
            with_allegiance(ent(OWNED_PET_ID, EntityKind::Pet, 4.0), SELF_NATION),
            with_allegiance(ent(PARTY_PET_ID, EntityKind::Pet, 4.0), SELF_NATION),
        ];
        s.snapshot.party = vec![party_member(PARTY_ID)];
        s
    }

    #[test]
    fn narrowed_raise_keeps_dead_targets_and_party_membership() {
        use ffxi_vocab::valid_target::TargetFlags;
        let mut scene = battle_scene();
        scene.snapshot.party.push(party_member(SELF_ID));
        let mut alliance = party_member(PARTY_PET_ID);
        alliance.party_no = 1;
        scene.snapshot.party.push(alliance);
        for id in [PARTY_ID, PARTY_PET_ID] {
            let entity = scene
                .snapshot
                .entities
                .iter_mut()
                .find(|e| e.id == id)
                .unwrap();
            entity.kind = EntityKind::Pc;
            entity.hp_pct = Some(0);
        }
        let raise = ffxi_vocab::spell_names::id_for("Raise").unwrap();
        for (mask, expected) in [
            (TargetFlags::PLAYER, PARTY_PET_ID),
            (TargetFlags::PLAYER_PARTY, PARTY_ID),
            (TargetFlags::PLAYER_ALLIANCE, PARTY_PET_ID),
        ] {
            let mode = open_sub_target_narrowed(
                SubTargetAction::Spell(raise),
                Some(TargetFlags(mask)),
                Some(PARTY_PET_ID),
                &mut scene,
                InputMode::World,
            );
            let Some(InputMode::SubTarget(st)) = mode else {
                panic!("Raise must retain its corpse candidates");
            };
            assert_eq!(st.candidate, Some(expected));
            assert!(TargetFlags(st.flags).contains(TargetFlags::PLAYER_DEAD));
        }
    }

    #[test]
    fn pick_sub_does_not_park_on_the_main_target() {
        let mut scene = battle_scene();
        let mode = open_sub_target(
            SubTargetAction::PickSub,
            Some(MOB_ID),
            &mut scene,
            InputMode::World,
        )
        .expect("the picker must open with candidates in range");
        let InputMode::SubTarget(st) = &mode else {
            panic!("expected the sub-target picker, got {mode:?}");
        };
        assert_ne!(
            st.candidate,
            Some(MOB_ID),
            "Switch Target must not park on the main target: the first Enter would be a no-op"
        );
    }

    #[test]
    fn pick_sub_confirm_sends_a_change_target_and_holds_the_picker() {
        let mut scene = battle_scene();
        let mode = open_sub_target(
            SubTargetAction::PickSub,
            Some(MOB_ID),
            &mut scene,
            InputMode::World,
        )
        .unwrap();
        let InputMode::SubTarget(mut st) = mode else {
            panic!("expected the sub-target picker");
        };
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let entities = scene.snapshot.entities.clone();
        let next = handle_sub_target_key(
            &Key::Enter,
            &Bindings::default(),
            &mut st,
            &mut scene,
            &entities,
            &cmd_tx,
        );
        assert!(
            next.is_none(),
            "confirm must hold the picker up for the server's 0x058: {next:?}"
        );
        let sent = cmd_rx
            .try_recv()
            .expect("confirm must send the ChangeTarget");
        assert!(
            matches!(
                sent,
                AgentCommand::Action {
                    target_id,
                    kind: ActionKind::ChangeTarget,
                    ..
                } if target_id == st.candidate.unwrap()
            ),
            "confirm must ask the server to move the battle target to the chosen candidate: {sent:?}"
        );
        assert_eq!(
            st.pending_switch, st.candidate,
            "the sent candidate must arm the answer wait"
        );
        assert!(st.pending_since.is_some());
    }

    /// The candidate sits inside the sub-target range, beyond the engage
    /// range: the local pre-check refuses and no command leaves the client.
    #[test]
    fn pick_sub_confirm_refuses_a_mob_beyond_the_engage_range() {
        let far_id: u32 = 0x0200_0003;
        let mut scene = battle_scene();
        scene
            .snapshot
            .entities
            .push(ent(far_id, EntityKind::Mob, 40.0));
        let mode = open_sub_target(
            SubTargetAction::PickSub,
            Some(MOB_ID),
            &mut scene,
            InputMode::World,
        )
        .unwrap();
        let InputMode::SubTarget(mut st) = mode else {
            panic!("expected the sub-target picker");
        };
        st.candidate = Some(far_id);
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let entities = scene.snapshot.entities.clone();
        let next = handle_sub_target_key(
            &Key::Enter,
            &Bindings::default(),
            &mut st,
            &mut scene,
            &entities,
            &cmd_tx,
        );
        assert!(
            next.is_none(),
            "a locally refused switch must keep the cursor up for the next candidate: {next:?}"
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "no command may leave the client for a doomed switch"
        );
        assert_eq!(
            st.pending_switch, None,
            "a refused candidate must not arm the wait"
        );
    }

    /// Enemy pets are valid Switch Target candidates, friendly pets are not,
    /// and the picker parks on the enemy pet, not a friendly one.
    #[test]
    fn switch_target_includes_enemy_pets_not_friendly_pets() {
        let scene = battle_scene();
        let flags = kuluu_render::sub_target::action_flags(SubTargetAction::PickSub);
        let ents = gather_sub_target_entities(&scene);
        let by_id = |id: u32| ents.iter().find(|e| e.id == id).expect("scene entity");
        assert!(kuluu_render::sub_target::entity_valid(
            flags,
            by_id(ENEMY_PET_ID)
        ));
        assert!(!kuluu_render::sub_target::entity_valid(
            flags,
            by_id(OWNED_PET_ID)
        ));
        assert!(!kuluu_render::sub_target::entity_valid(
            flags,
            by_id(PARTY_PET_ID)
        ));
        let mut scene = battle_scene();
        let mode = open_sub_target(
            SubTargetAction::PickSub,
            Some(MOB_ID),
            &mut scene,
            InputMode::World,
        )
        .expect("the picker must open with candidates in range");
        let InputMode::SubTarget(st) = &mode else {
            panic!("expected the sub-target picker, got {mode:?}");
        };
        assert_eq!(st.candidate, Some(ENEMY_PET_ID));
    }

    /// The retail "confirm on the current target" rule stays for actions:
    /// Cure on a valid party target parks on it, not on self.
    #[test]
    fn spell_prompt_still_parks_on_the_current_target() {
        let mut scene = battle_scene();
        let mode = open_sub_target(
            SubTargetAction::Spell(1),
            Some(PARTY_ID),
            &mut scene,
            InputMode::World,
        )
        .unwrap();
        let InputMode::SubTarget(st) = &mode else {
            panic!("expected the sub-target picker");
        };
        assert_eq!(st.candidate, Some(PARTY_ID));
    }

    /// Own-pet marking comes from the pet-sync targid (distinct wire targids
    /// per entity). A PET-flagged ability (Sic, 72) accepts the own pet; the
    /// ENEMY-only Switch Target does not.
    #[test]
    fn own_pet_is_marked_from_the_pet_sync_targid() {
        let mut scene = battle_scene();
        for (i, e) in scene.snapshot.entities.iter_mut().enumerate() {
            e.act_index = i as u16 + 1;
        }
        let owned_idx = scene
            .snapshot
            .entities
            .iter()
            .position(|e| e.id == OWNED_PET_ID)
            .expect("owned pet in scene");
        scene.snapshot.self_pet_targid = Some(scene.snapshot.entities[owned_idx].act_index);
        let ents = gather_sub_target_entities(&scene);
        let by_id = |id: u32| ents.iter().find(|e| e.id == id).expect("scene entity");
        assert!(by_id(OWNED_PET_ID).is_own_pet);
        assert!(!by_id(PARTY_PET_ID).is_own_pet);
        assert!(!by_id(ENEMY_PET_ID).is_own_pet);
        let flags = kuluu_render::sub_target::action_flags(SubTargetAction::Ability(72));
        assert!(kuluu_render::sub_target::entity_valid(
            flags,
            by_id(OWNED_PET_ID)
        ));
        let pick = kuluu_render::sub_target::action_flags(SubTargetAction::PickSub);
        assert!(!kuluu_render::sub_target::entity_valid(
            pick,
            by_id(OWNED_PET_ID)
        ));
    }

    #[test]
    fn pick_sub_cycling_cancels_the_answer_wait() {
        let mut scene = battle_scene();
        let mode = open_sub_target(
            SubTargetAction::PickSub,
            Some(MOB_ID),
            &mut scene,
            InputMode::World,
        )
        .unwrap();
        let InputMode::SubTarget(mut st) = mode else {
            panic!("expected the sub-target picker");
        };
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let entities = scene.snapshot.entities.clone();
        handle_sub_target_key(
            &Key::Enter,
            &Bindings::default(),
            &mut st,
            &mut scene,
            &entities,
            &cmd_tx,
        );
        assert!(
            st.pending_switch.is_some(),
            "confirm must arm the answer wait"
        );
        handle_sub_target_key(
            &Key::Tab,
            &Bindings::default(),
            &mut st,
            &mut scene,
            &entities,
            &cmd_tx,
        );
        assert_eq!(
            st.pending_switch, None,
            "cycling to another candidate must cancel the in-flight switch wait"
        );
        assert!(st.pending_since.is_none());
    }

    /// The picker holds while the 0x058
    /// (vendor/server/src/map/packets/s2c/0x058_assist.cpp) is in flight and
    /// closes the moment the main target lands on the sent candidate — the
    /// swap the frame shows is the server's commit, not our send.
    #[test]
    fn pick_sub_picker_closes_when_the_server_commits_the_candidate() {
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = cs_input_lock_tests::gate_app(cmd_tx);
        let mut st = kuluu_render::input_mode::SubTargetState::open(
            SubTargetAction::PickSub,
            0,
            InputMode::World,
        );
        st.candidate = Some(MOB2_ID);
        st.pending_switch = Some(MOB2_ID);
        st.pending_since = Some(std::time::Instant::now());
        app.world_mut().insert_resource(InputMode::SubTarget(st));
        app.update();
        assert!(
            matches!(
                *app.world().resource::<InputMode>(),
                InputMode::SubTarget(_)
            ),
            "the picker must hold while the server's 0x058 is in flight"
        );
        app.world_mut().resource_mut::<Target>().id = Some(MOB2_ID);
        app.update();
        assert!(
            matches!(*app.world().resource::<InputMode>(), InputMode::World),
            "the committed 0x058 must close the picker"
        );
    }

    /// No 0x058 (vendor/server/src/map/packets/s2c/0x058_assist.cpp), no
    /// rejection the client can see: the wait lapses and the picker closes on
    /// its own, leaving the main target where it was. The pending window is
    /// armed past the answer timeout: the 0x058 did not come.
    #[test]
    fn pick_sub_picker_lapses_when_no_answer_lands() {
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = cs_input_lock_tests::gate_app(cmd_tx);
        let mut st = kuluu_render::input_mode::SubTargetState::open(
            SubTargetAction::PickSub,
            0,
            InputMode::World,
        );
        st.candidate = Some(MOB2_ID);
        st.pending_switch = Some(MOB2_ID);
        st.pending_since = Some(std::time::Instant::now() - std::time::Duration::from_millis(600));
        app.world_mut().insert_resource(InputMode::SubTarget(st));
        app.update();
        assert!(
            matches!(*app.world().resource::<InputMode>(), InputMode::World),
            "a lapsed answer wait must close the picker on its own"
        );
        assert_eq!(
            app.world().resource::<Target>().id,
            None,
            "the main target must stay where it was"
        );
    }

    /// The main target dying while the switch-target picker is up makes the
    /// switch moot: the picker closes (the weapon sheathes on its own via the
    /// pose pass's active-target gate).
    #[test]
    fn pick_sub_picker_closes_when_the_main_target_dies() {
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut app = cs_input_lock_tests::gate_app(cmd_tx);
        let st = kuluu_render::input_mode::SubTargetState::open(
            SubTargetAction::PickSub,
            0,
            InputMode::World,
        );
        app.world_mut().resource_mut::<Target>().id = Some(MOB_ID);
        app.world_mut().insert_resource(InputMode::SubTarget(st));
        app.update();
        assert!(
            matches!(
                *app.world().resource::<InputMode>(),
                InputMode::SubTarget(_)
            ),
            "the picker must stay up while the main target is alive"
        );
        app.world_mut().resource_mut::<Target>().id = None;
        app.update();
        assert!(
            matches!(*app.world().resource::<InputMode>(), InputMode::World),
            "a dead main target must close the switch-target picker"
        );
    }
}
