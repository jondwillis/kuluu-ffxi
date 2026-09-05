use super::map_screen::handle_map_key;
use super::slash_apply::apply_keybind_update;
use super::*;

#[derive(Debug, Clone, PartialEq)]
enum MenuDispatch {
    CommandWithToast { cmd: AgentCommand, toast: String },

    OpenSubmenu(MenuKind),

    KeybindUpdate(KeybindUpdate),

    NotImplemented(String),
}

fn apply_graphics_cycle(cursor: usize, delta: i32, graphics: &mut kuluu_render::GraphicsSettings) {
    // The page carries two non-field action rows ("DLSS Config" under the DLSS
    // on/off row, "Reset to High" at the bottom), so the cursor slot does not
    // index GRAPHICS_FIELDS directly — resolve through the shared mapping.
    if let Some(field) = kuluu_render::hud::menu::graphics_field_at(cursor) {
        graphics.cycle(field, delta);
    }
}

/// Same shape for the DLSS Config submenu: slot -> DLSS_CONFIG_FIELDS. The
/// reset row sits one past the fields and is handled by the caller, so a
/// cursor there is a no-op here (get returns None), matching apply_graphics_cycle.
fn apply_graphics_dlss_cycle(
    cursor: usize,
    delta: i32,
    graphics: &mut kuluu_render::GraphicsSettings,
) {
    use kuluu_render::graphics_settings::DLSS_CONFIG_FIELDS;
    if let Some(&field) = DLSS_CONFIG_FIELDS.get(cursor) {
        graphics.cycle(field, delta);
    }
}

fn resolve_menu_entry(kind: MenuKind, label: &str) -> MenuDispatch {
    use kuluu_render::hud::menu::{COMM_EMOTE_LIST, ROOT_LOG_OUT, ROOT_SHUT_DOWN};
    match (kind, label) {
        (MenuKind::Communication, l) if l == COMM_EMOTE_LIST => {
            MenuDispatch::OpenSubmenu(MenuKind::EmoteList)
        }
        (MenuKind::Root, ROOT_LOG_OUT) => MenuDispatch::CommandWithToast {
            cmd: AgentCommand::ReqLogout {
                kind: ReqLogoutKind::LogoutToggle,
            },
            toast: "[menu] Log Out requested (~30s; instant in Mog House). \
                    Select again or `/logout off` to cancel."
                .into(),
        },

        (MenuKind::Root, ROOT_SHUT_DOWN) => MenuDispatch::CommandWithToast {
            cmd: AgentCommand::ReqLogout {
                kind: ReqLogoutKind::ShutdownToggle,
            },
            toast: "[menu] Shut Down requested (~30s; instant in Mog House). \
                    Select again or `/shutdown off` to cancel."
                .into(),
        },

        // The Map screen is a bespoke pane (no generic right-pane preview via
        // root_child_kind), so it needs its own drill arm ahead of the catch-all.
        (MenuKind::Root, "Map") => MenuDispatch::OpenSubmenu(MenuKind::Map),

        // Root categories that drill into a browsable submenu share their
        // mapping with the right-pane preview (single source of truth).
        (MenuKind::Root, label) => match kuluu_render::hud::menu::root_child_kind(label) {
            Some(submenu) => MenuDispatch::OpenSubmenu(submenu),
            None => MenuDispatch::NotImplemented(label.to_string()),
        },

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

const EQUIP_SLOT_INDEX_MAX: u8 = (kuluu_render::equip_slot::EquipmentIndex::ALL.len() - 1) as u8;

pub(super) fn confirm_menu_at_cursor(
    bindings: &mut Bindings,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    keybinds_state: &mut KeybindsStateRes,
    graphics: &mut kuluu_render::GraphicsSettings,
    status_profile_open: &mut kuluu_render::hud::status_panel::StatusProfileOpen,
    hud_panels: &mut kuluu_render::hud::HudPanels,
    net_status: &mut kuluu_render::hud::network_status::NetStatusVisible,
    audio_mute: &mut kuluu_render::audio::AudioMuteState,
    vana_clock: &kuluu_render::vana_time::VanaClock,
    vana_clock_visible: &mut kuluu_render::hud::vana_clock::VanaClockVisible,
    dynamic: &kuluu_render::hud::menu::DynamicMenu,
    target_id: Option<u32>,
    self_pos: kuluu_snapshot::Vec3,
) -> Option<InputMode> {
    let (kind, cursor) = {
        let level = stack.current()?;
        (level.kind, level.cursor)
    };

    if matches!(kind, MenuKind::Debug) {
        let label = kuluu_render::hud::menu::entry_label(kind, cursor, dynamic);
        // Volume is a 0..=100 number row adjusted with Left/Right, not a toggle.
        // Pressing the confirm key on it should do nothing rather than fall
        // through to toggle_debug_panel's unknown-entry branch.
        if label == kuluu_render::hud::menu::DEBUG_VOLUME {
            return None;
        }
        // Retail+ section rows live in GraphicsSettings (persisted), not
        // HudPanels — handle them before the panel toggles.
        if handle_retail_plus_row(label, graphics, scene_state) {
            return None;
        }
        toggle_debug_panel(
            label,
            hud_panels,
            net_status,
            audio_mute,
            self_pos,
            scene_state,
        );
        return None;
    }

    if matches!(kind, MenuKind::Root)
        && kuluu_render::hud::menu::entry_label(kind, cursor, dynamic)
            == kuluu_render::hud::menu::ROOT_CURRENT_TIME
    {
        activate_current_time(vana_clock, vana_clock_visible, scene_state);
        // Mirrors the Debug toggles: the menu stays open (provisional pending
        // retail capture, bead kuluu-y5hq retail_unknowns).
        return None;
    }

    if matches!(kind, MenuKind::Status) {
        use kuluu_render::hud::status_panel::{StatusEntryKind, STATUS_ENTRIES};
        let entry = STATUS_ENTRIES.get(cursor)?;
        match entry.kind {
            StatusEntryKind::Profile => {
                status_profile_open.0 = true;
            }
            StatusEntryKind::PlayTime => {
                let line =
                    kuluu_render::hud::status_panel::play_time_chat_line(&scene_state.snapshot);
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
        if cursor == kuluu_render::hud::menu::GRAPHICS_RESET_SLOT {
            graphics.reset_to_default();
            push_system_chat_line(scene_state, "[menu] Graphics reset to High".into());
        } else if cursor == kuluu_render::hud::menu::GRAPHICS_DLSS_CONFIG_SLOT {
            stack.push(MenuKind::GraphicsDlss);
        } else {
            apply_graphics_cycle(cursor, 1, graphics);
        }
        return None;
    }

    if matches!(kind, MenuKind::GraphicsDlss) {
        if cursor == kuluu_render::hud::menu::GRAPHICS_DLSS_RESET_SLOT {
            graphics.reset_dlss_config();
            push_system_chat_line(scene_state, "[menu] DLSS config reset to defaults".into());
        } else {
            apply_graphics_dlss_cycle(cursor, 1, graphics);
        }
        return None;
    }

    if matches!(kind, MenuKind::Equipment) {
        let slot = (cursor as u8).min(EQUIP_SLOT_INDEX_MAX);
        stack.push(MenuKind::EquipSlot(slot));
        return None;
    }

    if kuluu_render::hud::menu::is_dynamic(kind) {
        if let Some(action) = kuluu_render::hud::menu::entry_action(kind, cursor, dynamic) {
            use kuluu_render::hud::menu::DynamicMenuAction as A;
            if let A::OpenItemAction {
                container,
                index,
                item_no,
            } = action
            {
                stack.push(MenuKind::ItemAction {
                    container,
                    index,
                    item_no,
                });
                return None;
            }
            // Retail's key-item detail pane needs a description DAT not yet
            // identified (bead kuluu-h7x retail_unknowns); echo the name and
            // keep the list open.
            if let A::KeyItem { id } = action {
                push_system_chat_line(
                    scene_state,
                    format!(
                        "Key item: {}.",
                        kuluu_render::hud::menu::key_item_row_label(id, true)
                    ),
                );
                return None;
            }
            if let Some(sub_action) = sub_target_action_for(action) {
                if !selected_target_valid(sub_action, target_id, scene_state) {
                    // No valid target selected: retail's sub-target confirm step
                    // fires the action only after the flashing cursor is confirmed.
                    // Esc restores this menu with its cursor intact.
                    let return_to = InputMode::Menu(stack.clone());
                    return open_sub_target(sub_action, target_id, scene_state, return_to);
                }
            }
            let moved = matches!(action, A::MoveItem { .. });
            let entities = scene_state.snapshot.entities.clone();
            dispatch_dynamic_menu_action(
                action,
                target_id,
                self_pos,
                &entities,
                cmd_tx,
                scene_state,
            );
            // Retail keeps the equip list up after a gear change so the player
            // can keep swapping (or re-select to unequip), and keeps the bag
            // open after moving an item so a sort/move session flows; the
            // one-shot action menus (Magic/Abilities/item Use) close back to
            // the world.
            return if matches!(kind, MenuKind::EquipSlot(_)) {
                None
            } else if moved {
                stack.pop();
                None
            } else {
                Some(InputMode::World)
            };
        }

        push_system_chat_line(scene_state, format!("[menu] {kind:?} list is empty"));
        return None;
    }
    let label = kuluu_render::hud::menu::entry_label(kind, cursor, dynamic);
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
            // Refresh the job-emote/chair unlock bits whenever the Emote List
            // opens (c2s 0x119 → s2c 0x11A gates the Job row).
            if submenu == MenuKind::EmoteList {
                let _ = cmd_tx.try_send(AgentCommand::RequestEmoteList);
            }
            // The Map screen opens on its command submenu; the wide-scan request
            // (0x0F4) fires only when the player selects "Wide Scan", not on open.
            // `reset_map_screen_on_open` clears the submode as the screen appears.
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

fn activate_current_time(
    vana_clock: &kuluu_render::vana_time::VanaClock,
    vana_clock_visible: &mut kuluu_render::hud::vana_clock::VanaClockVisible,
    scene_state: &mut SceneState,
) {
    vana_clock_visible.0 = !vana_clock_visible.0;
    for line in kuluu_render::hud::vana_clock::current_time_chat_lines(vana_clock) {
        push_system_chat_line(scene_state, line);
    }
}

/// Retail+ section rows (dev-only Debug menu). Returns true when the label is
/// one of them and it was handled here — the caller must not fall through to
/// `toggle_debug_panel`. The live toggles flip GraphicsSettings fields, so
/// `persist_graphics_on_change` writes graphics.json automatically. Mob HP
/// Under / Job Display only exist in enhanced builds (their rows are absent
/// from DEBUG_ENTRIES without their feature).
fn handle_retail_plus_row(
    label: &str,
    graphics: &mut kuluu_render::GraphicsSettings,
    scene_state: &mut SceneState,
) -> bool {
    #[cfg(feature = "enhanced-job-display")]
    use kuluu_render::hud::menu::RETAIL_JOB_DISPLAY;
    #[cfg(feature = "enhanced-mob-hp-under")]
    use kuluu_render::hud::menu::RETAIL_MOB_HP_UNDER;
    use kuluu_render::hud::menu::{DEBUG_RETAIL_LABEL, DEBUG_RETAIL_SEPARATOR, RETAIL_DLSS_MENU};
    match label {
        // Section chrome: no state, no banner.
        DEBUG_RETAIL_SEPARATOR | DEBUG_RETAIL_LABEL => true,
        RETAIL_DLSS_MENU => {
            graphics.dlss_menu_enabled = !graphics.dlss_menu_enabled;
            if graphics.dlss_menu_enabled && !graphics.dlss_supported {
                // The user just asked for DLSS in the Graphics menu on a
                // machine/build that can't run it (DLLs missing, no RTX/Vulkan,
                // or built without the dlss feature). Say so loudly — the row
                // will keep reading N/A until the runtime files are present.
                tracing::error!(
                    "[menu] DLSS enabled in menu but the NVIDIA DLSS runtime files were not found \
                     (DLSS DLLs missing, no RTX/Vulkan support, or this build lacks the dlss feature) — \
                     Graphics menu will show N/A"
                );
            }
            push_system_chat_line(
                scene_state,
                format!(
                    "[menu] {label}: {}",
                    if graphics.dlss_menu_enabled {
                        "on"
                    } else {
                        "off"
                    }
                ),
            );
            true
        }
        #[cfg(feature = "enhanced-mob-hp-under")]
        RETAIL_MOB_HP_UNDER => {
            graphics.mob_hp_under = !graphics.mob_hp_under;
            push_system_chat_line(
                scene_state,
                format!(
                    "[menu] {label}: {}",
                    if graphics.mob_hp_under { "on" } else { "off" }
                ),
            );
            true
        }
        #[cfg(feature = "enhanced-job-display")]
        RETAIL_JOB_DISPLAY => {
            graphics.job_display = !graphics.job_display;
            push_system_chat_line(
                scene_state,
                format!(
                    "[menu] {label}: {}",
                    if graphics.job_display { "on" } else { "off" }
                ),
            );
            true
        }
        _ => false,
    }
}

fn toggle_debug_panel(
    label: &str,
    hud_panels: &mut kuluu_render::hud::HudPanels,
    net_status: &mut kuluu_render::hud::network_status::NetStatusVisible,
    audio_mute: &mut kuluu_render::audio::AudioMuteState,
    self_pos: kuluu_snapshot::Vec3,
    scene_state: &mut SceneState,
) {
    use kuluu_render::hud::menu::{
        DEBUG_GRAPHICS_DEBUG, DEBUG_MESH, DEBUG_NAMEPLATES, DEBUG_NET_STATUS, DEBUG_NOCLIP,
        DEBUG_PERF, DEBUG_POSITION_LOG, DEBUG_PRINT_POS, DEBUG_SOUND, DEBUG_STAIR_DRAW,
        DEBUG_STAIR_STATUS, DEBUG_TARGET_CYCLE, DEBUG_UI_SETTINGS,
    };

    // Print Pos is a button, not a toggle: fire and return before the
    // on/off banner below. Prints self wire coords to the system chat.
    if label == DEBUG_PRINT_POS {
        push_system_chat_line(
            scene_state,
            format!(
                "[debug] pos: x={:.3} y={:.3} z={:.3}",
                self_pos.x, self_pos.y, self_pos.z,
            ),
        );
        return;
    }

    let on = match label {
        DEBUG_PERF => {
            hud_panels.perf = !hud_panels.perf;
            hud_panels.perf
        }
        DEBUG_TARGET_CYCLE => {
            hud_panels.target_cycle = !hud_panels.target_cycle;
            hud_panels.target_cycle
        }
        DEBUG_MESH => {
            hud_panels.mesh_debug = !hud_panels.mesh_debug;
            hud_panels.mesh_debug
        }
        DEBUG_NOCLIP => {
            hud_panels.noclip = !hud_panels.noclip;
            hud_panels.noclip
        }
        DEBUG_STAIR_DRAW => {
            hud_panels.stair_draw = !hud_panels.stair_draw;
            hud_panels.stair_draw
        }
        DEBUG_STAIR_STATUS => {
            hud_panels.stair_debug = !hud_panels.stair_debug;
            hud_panels.stair_debug
        }
        DEBUG_GRAPHICS_DEBUG => {
            hud_panels.graphics_debug = !hud_panels.graphics_debug;
            hud_panels.graphics_debug
        }
        DEBUG_POSITION_LOG => {
            hud_panels.position_log = !hud_panels.position_log;
            hud_panels.position_log
        }
        DEBUG_NAMEPLATES => {
            hud_panels.nameplate_debug = !hud_panels.nameplate_debug;
            hud_panels.nameplate_debug
        }
        DEBUG_UI_SETTINGS => {
            hud_panels.ui_settings = !hud_panels.ui_settings;
            hud_panels.ui_settings
        }
        DEBUG_NET_STATUS => {
            net_status.0 = !net_status.0;
            net_status.0
        }
        DEBUG_SOUND => {
            // Toggle master: if either category is currently unmuted,
            // sound reads as ON, so a click MUTES both. Otherwise UNMUTE.
            let was_on = !(audio_mute.bgm && audio_mute.sfx);
            audio_mute.bgm = was_on;
            audio_mute.sfx = was_on;
            !was_on
        }
        other => {
            push_system_chat_line(scene_state, format!("[menu] Debug: unknown `{other}`"));
            return;
        }
    };
    push_system_chat_line(
        scene_state,
        format!("[menu] {label}: {}", if on { "on" } else { "off" }),
    );
}

pub(super) fn handle_menu_key(
    key: &Key,
    key_code: KeyCode,
    bindings: &mut Bindings,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    keybinds_state: &mut KeybindsStateRes,
    graphics: &mut kuluu_render::GraphicsSettings,
    status_profile_open: &mut kuluu_render::hud::status_panel::StatusProfileOpen,
    hud_panels: &mut kuluu_render::hud::HudPanels,
    net_status: &mut kuluu_render::hud::network_status::NetStatusVisible,
    audio_mute: &mut kuluu_render::audio::AudioMuteState,
    vana_clock: &kuluu_render::vana_time::VanaClock,
    vana_clock_visible: &mut kuluu_render::hud::vana_clock::VanaClockVisible,
    sort_options: &mut kuluu_render::hud::item_detail::SortOptions,
    item_menu_focus: &mut kuluu_render::hud::item_detail::ItemMenuFocus,
    item_bag: &mut kuluu_render::hud::item_screen::ItemScreenContainer,
    dynamic: &kuluu_render::hud::menu::DynamicMenu,
    target_id: Option<u32>,
    self_pos: kuluu_snapshot::Vec3,
    map_state: &mut kuluu_render::hud::map_screen::MapScreenState,
    // `Mut` (not `&mut` off a call-site `ResMut` deref, which flags the
    // resource changed on every menu key): dropped untouched on non-Map paths,
    // change detection fires only when the Map screen mutates a marker
    // (kuluu-df0x).
    map_markers: Mut<kuluu_render::hud::map_screen::MapMarkers>,
    map_view: &kuluu_render::hud::map_screen::MapView,
    minimap_state: &kuluu_render::minimap::MinimapState,
) -> Option<InputMode> {
    let top_kind = stack.current()?.kind;

    // The very key that opened this menu (Action::OpenMenu on "-", handled by the
    // input system chained just before this one) also arrives here on the same
    // frame; absorb it once so it doesn't immediately flip Root to page 2
    // (kuluu-bi1s.2). Clear the one-shot flag on the first menu key regardless.
    if stack.take_absorb_open_minus() && key_code == KeyCode::Minus {
        return None;
    }

    // The Map screen is a bespoke full-screen surface (full-screen map + a
    // top-right command submenu drilling into Markers/Wide Scan/Change Map),
    // with its own submode navigation, so it intercepts before generic routing.
    if top_kind == MenuKind::Map {
        return handle_map_key(
            key,
            key_code,
            bindings,
            stack,
            scene_state,
            cmd_tx,
            map_state,
            map_markers,
            map_view,
            minimap_state,
        );
    }
    let (kind, cursor) = {
        let level = stack.current()?;
        (level.kind, level.cursor)
    };
    let entry_count = kuluu_render::hud::menu::entry_count(kind, dynamic);

    // Menu context (not text input), so reading the raw keycode is correct.
    // "-" flips the Command menu's two pages (retail HorizonXI); single-list
    // submenus have no pages, and the Map screen handles "-" in its own path.
    if key_code == KeyCode::Minus {
        if kind == MenuKind::Root {
            if let Some(level) = stack.current_mut() {
                level.cursor = kuluu_render::hud::menu::root_other_page_cursor(level.cursor);
            }
        }
        return None;
    }

    // Root Command menu paging: Left/Right flip pages (like "-"); Up/Down wrap
    // within the current page so navigation never crosses a page boundary.
    if kind == MenuKind::Root {
        let (start, end) = kuluu_render::hud::menu::root_page_bounds(cursor);
        if bindings.matches_logical(Action::NavLeft, key)
            || bindings.matches_logical(Action::NavRight, key)
        {
            if let Some(level) = stack.current_mut() {
                level.cursor = kuluu_render::hud::menu::root_other_page_cursor(level.cursor);
            }
            return None;
        }
        if bindings.matches_logical(Action::NavUp, key) {
            let level = stack.current_mut()?;
            level.cursor = if cursor <= start { end - 1 } else { cursor - 1 };
            return None;
        }
        if bindings.matches_logical(Action::NavDown, key) {
            let level = stack.current_mut()?;
            let next = cursor + 1;
            level.cursor = if next >= end { start } else { next };
            return None;
        }
    }

    // The Items window is a stack of panes: one per accessible bag plus the
    // sort-options box. Retail's "Select active window" key (F in the compact
    // presets, Numpad + on the full keyboard) steps focus through them in
    // order, while NavLeft/NavRight page the item list a viewport at a time —
    // matching the retail client, which never repurposes left/right for pane
    // changes.
    if matches!(kind, MenuKind::Items) {
        use kuluu_render::hud::item_detail::{sort_pane_key, SortPaneKey};
        if bindings.matches_logical(Action::SelectActiveWindow, key) {
            if kuluu_render::hud::item_screen::select_active_window(
                &scene_state.snapshot,
                item_bag,
                item_menu_focus,
                sort_options,
            )
            .is_some()
            {
                if let Some(level) = stack.current_mut() {
                    level.cursor = 0;
                }
            }
            return None;
        }
        if item_menu_focus.sort_focused() {
            let pane_key = if bindings.matches_logical(Action::NavUp, key) {
                SortPaneKey::Up
            } else if bindings.matches_logical(Action::NavDown, key) {
                SortPaneKey::Down
            } else if bindings.matches_logical(Action::NavConfirm, key) {
                SortPaneKey::Confirm
            } else if bindings.matches_logical(Action::NavLeft, key)
                || bindings.matches_logical(Action::NavCancel, key)
            {
                SortPaneKey::Exit
            } else {
                // Swallow any other key so it can't leak into list navigation.
                SortPaneKey::Other
            };
            if sort_pane_key(item_menu_focus, sort_options, pane_key).is_some() {
                if let Err(e) = cmd_tx.try_send(AgentCommand::StackInventory {
                    container: ffxi_proto::map::container::LOC_INVENTORY,
                }) {
                    push_system_chat_line(scene_state, format!("sort dropped (channel): {e}"));
                }
            }
            return None;
        }
        // Retail pages the item list with left/right: one viewport per press,
        // clamped at the ends (no wrap).
        let page = if bindings.matches_logical(Action::NavLeft, key) {
            Some(false)
        } else if bindings.matches_logical(Action::NavRight, key) {
            Some(true)
        } else {
            None
        };
        if let Some(forward) = page {
            let rows = kuluu_render::hud::menu::list_page_rows(kind);
            if let Some(level) = stack.current_mut() {
                level.cursor =
                    kuluu_render::hud::menu::page_cursor(level.cursor, entry_count, rows, forward);
            }
            return None;
        }
    }

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

    if matches!(kind, MenuKind::GraphicsDlss) {
        if bindings.matches_logical(Action::NavLeft, key) {
            apply_graphics_dlss_cycle(cursor, -1, graphics);
            return None;
        }
        if bindings.matches_logical(Action::NavRight, key) {
            apply_graphics_dlss_cycle(cursor, 1, graphics);
            return None;
        }
    }

    // Debug menu: the Volume row is a 0..=100 number adjusted with Left/Right.
    // Every other Debug row is a toggle handled on the confirm key, so only
    // Volume consumes arrows here; anything else falls through to normal list
    // navigation.
    if matches!(kind, MenuKind::Debug) {
        let label = kuluu_render::hud::menu::entry_label(kind, cursor, dynamic);
        if label == kuluu_render::hud::menu::DEBUG_VOLUME {
            if bindings.matches_logical(Action::NavLeft, key) {
                audio_mute.cycle_master(-1);
                return None;
            }
            if bindings.matches_logical(Action::NavRight, key) {
                audio_mute.cycle_master(1);
                return None;
            }
        }
    }

    // The Equipment screen is a 2D retail icon grid: arrows move between grid
    // cells (cursor stays an internal slot index), not down a linear list.
    if matches!(kind, MenuKind::Equipment) {
        let delta = if bindings.matches_logical(Action::NavLeft, key) {
            Some((-1, 0))
        } else if bindings.matches_logical(Action::NavRight, key) {
            Some((1, 0))
        } else if bindings.matches_logical(Action::NavUp, key) {
            Some((0, -1))
        } else if bindings.matches_logical(Action::NavDown, key) {
            Some((0, 1))
        } else {
            None
        };
        if let Some((dx, dy)) = delta {
            let level = stack.current_mut()?;
            level.cursor =
                kuluu_render::hud::equipment_screen::grid_move(level.cursor as u8, dx, dy) as usize;
            return None;
        }
    }

    // Retail pages every other vertical list menu (the action-ring Usable list,
    // Magic/Abilities/Key Items/Emotes, the equip-slot picker) with Left/Right,
    // one visible page per press, clamped at the ends. Items handled its own
    // paging above; Graphics (value cycles) and the Equipment grid consumed
    // Left/Right in their blocks.
    if kuluu_render::hud::menu::is_dynamic(kind) {
        let page = if bindings.matches_logical(Action::NavLeft, key) {
            Some(false)
        } else if bindings.matches_logical(Action::NavRight, key) {
            Some(true)
        } else {
            None
        };
        if let Some(forward) = page {
            let rows = kuluu_render::hud::menu::list_page_rows(kind);
            let level = stack.current_mut()?;
            level.cursor =
                kuluu_render::hud::menu::page_cursor(level.cursor, entry_count, rows, forward);
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
            hud_panels,
            net_status,
            audio_mute,
            vana_clock,
            vana_clock_visible,
            dynamic,
            target_id,
            self_pos,
        );
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        if matches!(kind, MenuKind::Status) {
            status_profile_open.0 = false;
        }
        // Cancel pops one level; from Root it closes back to the world.
        return if !stack.pop() {
            Some(InputMode::World)
        } else {
            None
        };
    }
    None
}

#[cfg(test)]
mod menu_key_tests {
    use super::*;
    use crate::keybinds_store::KeybindsStore;
    use bevy::ecs::world::World;
    use kuluu_render::hud::map_screen::{MapMarkers, MapScreenState, MapSubMode, MapView};
    use kuluu_render::input_mode::Pane;
    use kuluu_render::minimap::{MinimapAabb, MinimapState};

    struct Harness {
        bindings: Bindings,
        scene_state: SceneState,
        keybinds_state: KeybindsStateRes,
        graphics: kuluu_render::GraphicsSettings,
        status_profile_open: kuluu_render::hud::status_panel::StatusProfileOpen,
        hud_panels: kuluu_render::hud::HudPanels,
        net_status: kuluu_render::hud::network_status::NetStatusVisible,
        audio_mute: kuluu_render::audio::AudioMuteState,
        vana_clock: kuluu_render::vana_time::VanaClock,
        vana_clock_visible: kuluu_render::hud::vana_clock::VanaClockVisible,
        sort_options: kuluu_render::hud::item_detail::SortOptions,
        item_menu_focus: kuluu_render::hud::item_detail::ItemMenuFocus,
        item_bag: kuluu_render::hud::item_screen::ItemScreenContainer,
        dynamic: kuluu_render::hud::menu::DynamicMenu,
        map_state: MapScreenState,
        map_view: MapView,
        minimap_state: MinimapState,
        cmd_tx: Sender<AgentCommand>,
        _cmd_rx: tokio::sync::mpsc::Receiver<AgentCommand>,
    }

    impl Harness {
        fn new() -> Self {
            let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel(8);
            Self {
                bindings: Bindings::default(),
                scene_state: SceneState::default(),
                keybinds_state: KeybindsStateRes {
                    store: KeybindsStore::new(
                        std::env::temp_dir().join("kuluu-menu-key-tests-unused.json"),
                    ),
                    persisted: Default::default(),
                },
                graphics: Default::default(),
                status_profile_open: Default::default(),
                hud_panels: Default::default(),
                net_status: Default::default(),
                audio_mute: Default::default(),
                vana_clock: kuluu_render::vana_time::VanaClock::anchored_at_hour(12.0),
                vana_clock_visible: Default::default(),
                sort_options: Default::default(),
                item_menu_focus: Default::default(),
                item_bag: Default::default(),
                dynamic: Default::default(),
                map_state: MapScreenState::default(),
                map_view: MapView::default(),
                minimap_state: MinimapState::default(),
                cmd_tx,
                _cmd_rx,
            }
        }

        fn key(
            &mut self,
            key: &Key,
            key_code: KeyCode,
            stack: &mut MenuStack,
            map_markers: Mut<MapMarkers>,
        ) -> Option<InputMode> {
            handle_menu_key(
                key,
                key_code,
                &mut self.bindings,
                stack,
                &mut self.scene_state,
                &self.cmd_tx,
                &mut self.keybinds_state,
                &mut self.graphics,
                &mut self.status_profile_open,
                &mut self.hud_panels,
                &mut self.net_status,
                &mut self.audio_mute,
                &self.vana_clock,
                &mut self.vana_clock_visible,
                &mut self.sort_options,
                &mut self.item_menu_focus,
                &mut self.item_bag,
                &self.dynamic,
                None,
                kuluu_snapshot::Vec3::default(),
                &mut self.map_state,
                map_markers,
                &self.map_view,
                &self.minimap_state,
            )
        }
    }

    fn marker_world() -> World {
        let mut world = World::new();
        world.insert_resource(MapMarkers::default());
        world.clear_trackers();
        world
    }

    /// kuluu-ce6z: the cursor is read from `stack.current()`, so writes must go
    /// to `current_mut()` too. With `active_pane = Pane::Left` on a depth-2
    /// stack, `active_level_mut()` resolves to the PARENT level and a NavDown
    /// computed from the top level's state would corrupt the parent's cursor.
    #[test]
    fn nav_down_writes_the_level_it_read_even_with_left_pane_active() {
        const PARENT_CURSOR_SENTINEL: usize = 3;
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Config);
        stack.levels[0].cursor = PARENT_CURSOR_SENTINEL;
        stack.active_pane = Pane::Left;

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowDown, KeyCode::ArrowDown, &mut stack, markers);

        assert_eq!(stack.levels[1].cursor, 1, "top level cursor advances");
        assert_eq!(
            stack.levels[0].cursor, PARENT_CURSOR_SENTINEL,
            "parent level cursor is untouched"
        );
        assert_eq!(stack.active_pane, Pane::Left, "pane not altered by nav");
    }

    /// kuluu-df0x: navigating a non-Map menu must not flag MapMarkers changed
    /// (marker_store::sync_markers rewrites markers.json on is_changed).
    #[test]
    fn non_map_menu_key_does_not_flag_markers_changed() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowDown, KeyCode::ArrowDown, &mut stack, markers);

        assert!(
            !world.is_resource_changed::<MapMarkers>(),
            "menu navigation must not dirty MapMarkers"
        );
    }

    #[test]
    fn placing_a_marker_flags_markers_changed() {
        const ZONE: u16 = 231;
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Map);
        stack.take_absorb_open_minus();

        harness.scene_state.snapshot.zone_id = Some(ZONE);
        harness.map_state.mode = MapSubMode::Markers;
        harness.map_state.map_cursor = Some(bevy::math::Vec2::splat(0.5));
        harness.map_state.marker_entry = Some("Camp".into());
        harness.map_view.visible_aabb = Some(MinimapAabb {
            min: bevy::math::Vec2::new(-100.0, -100.0),
            max: bevy::math::Vec2::new(100.0, 100.0),
        });

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);

        assert!(
            world.is_resource_changed::<MapMarkers>(),
            "confirming a named marker must dirty MapMarkers so it persists"
        );
        let markers = world.resource::<MapMarkers>();
        assert_eq!(markers.for_zone(ZONE).len(), 1);
        assert_eq!(markers.for_zone(ZONE)[0].label, "Camp");
    }

    /// kuluu-kzxp: Period/Comma zoom the full-screen map on default binds. The
    /// logical-key path never resolves non-letter printables, so the zoom gate
    /// must match on the raw keycode.
    #[test]
    fn default_zoom_keys_zoom_the_map_screen() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Map);
        stack.take_absorb_open_minus();
        assert_eq!(harness.map_state.zoom_radius, None, "opens fit-to-zone");

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(
            &Key::Character(".".into()),
            KeyCode::Period,
            &mut stack,
            markers,
        );
        let zoomed_in = harness.map_state.zoom_radius;
        assert!(zoomed_in.is_some(), "Period zooms in from fit-to-zone");

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(
            &Key::Character(",".into()),
            KeyCode::Comma,
            &mut stack,
            markers,
        );
        assert_eq!(
            harness.map_state.zoom_radius, None,
            "Comma zooms back out to fit-to-zone"
        );
    }

    #[test]
    fn typing_punctuation_into_a_marker_label_does_not_zoom() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Map);
        stack.take_absorb_open_minus();
        harness.map_state.mode = MapSubMode::Markers;
        harness.map_state.marker_entry = Some("E-8".to_string());

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(
            &Key::Character(",".into()),
            KeyCode::Comma,
            &mut stack,
            markers,
        );
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(
            &Key::Character(".".into()),
            KeyCode::Period,
            &mut stack,
            markers,
        );
        assert_eq!(harness.map_state.marker_entry.as_deref(), Some("E-8,."));
        assert_eq!(
            harness.map_state.zoom_radius, None,
            "zoom binds must not fire while a marker label is being typed"
        );
    }
}

#[cfg(test)]
mod menu_dispatch_tests {
    use super::*;

    #[test]
    fn log_out_dispatches_reqlogout_with_toast() {
        use kuluu_render::hud::menu::ROOT_LOG_OUT;
        match resolve_menu_entry(MenuKind::Root, ROOT_LOG_OUT) {
            MenuDispatch::CommandWithToast { cmd, toast } => {
                assert_eq!(
                    cmd,
                    AgentCommand::ReqLogout {
                        kind: ReqLogoutKind::LogoutToggle,
                    }
                );
                assert!(
                    toast.to_lowercase().contains("log out"),
                    "toast should mention log out, got {toast:?}"
                );
            }
            other => panic!("expected CommandWithToast for Log Out, got {other:?}"),
        }
    }

    #[test]
    fn shut_down_dispatches_shutdown_reqlogout_with_toast() {
        use kuluu_render::hud::menu::ROOT_SHUT_DOWN;
        match resolve_menu_entry(MenuKind::Root, ROOT_SHUT_DOWN) {
            MenuDispatch::CommandWithToast { cmd, toast } => {
                assert_eq!(
                    cmd,
                    AgentCommand::ReqLogout {
                        kind: ReqLogoutKind::ShutdownToggle,
                    }
                );
                assert!(
                    toast.to_lowercase().contains("shut down"),
                    "toast should mention shut down, got {toast:?}"
                );
            }
            other => panic!("expected CommandWithToast for Shut Down, got {other:?}"),
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

    /// The right-pane preview (`menu::root_child_kind`) and the drill dispatch
    /// share one Root → submenu mapping; pin that they can't drift apart.
    #[test]
    fn root_drill_matches_preview_child_kind() {
        use kuluu_render::hud::menu::{self, ROOT_LOG_OUT, ROOT_SHUT_DOWN};
        for &label in menu::root_entries() {
            // Log Out / Shut Down fire commands, not a browsable submenu.
            if label == ROOT_LOG_OUT || label == ROOT_SHUT_DOWN {
                continue;
            }
            match (
                resolve_menu_entry(MenuKind::Root, label),
                menu::root_child_kind(label),
            ) {
                (MenuDispatch::OpenSubmenu(dispatched), Some(preview)) => {
                    assert_eq!(dispatched, preview, "{label} drill vs preview drift");
                }
                // A drill with no right-pane preview is only legal when it opens
                // a bespoke full-screen menu (e.g. Map), which renders its own
                // panes instead of the generic preview.
                (MenuDispatch::OpenSubmenu(dispatched), None) => {
                    assert!(
                        menu::renders_bespoke_screen(dispatched),
                        "{label}: preview-less drill into non-bespoke {dispatched:?}"
                    );
                }
                (MenuDispatch::NotImplemented(_), None) => {}
                (dispatch, preview) => {
                    panic!("{label}: dispatch {dispatch:?} disagrees with preview {preview:?}")
                }
            }
        }
    }

    #[test]
    fn current_time_toggles_widget_and_prints_both_time_lines() {
        use kuluu_render::hud::vana_clock::{
            VanaClockVisible, EARTH_TIME_LINE_PREFIX, VANA_TIME_LINE_PREFIX,
        };
        let clock = kuluu_render::vana_time::VanaClock::anchored_at_hour(12.0);
        let mut visible = VanaClockVisible::default();
        let mut scene_state = SceneState::default();

        activate_current_time(&clock, &mut visible, &mut scene_state);
        assert!(
            !visible.0,
            "default-visible widget hides on first activation"
        );
        let lines: Vec<&str> = scene_state
            .local_toasts
            .iter()
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with(VANA_TIME_LINE_PREFIX), "{lines:?}");
        assert!(lines[1].starts_with(EARTH_TIME_LINE_PREFIX), "{lines:?}");

        activate_current_time(&clock, &mut visible, &mut scene_state);
        assert!(visible.0, "second activation shows the widget again");
    }

    #[test]
    fn current_time_never_reaches_resolve_wired() {
        // confirm_menu_at_cursor intercepts ROOT_CURRENT_TIME (a
        // resource-touching entry) before resolve_menu_entry; this pins the
        // fallback so a lost wiring degrades to a visible "not implemented"
        // chat line rather than silently dispatching something else.
        use kuluu_render::hud::menu::ROOT_CURRENT_TIME;
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, ROOT_CURRENT_TIME),
            MenuDispatch::NotImplemented(ROOT_CURRENT_TIME.into()),
        );
    }

    /// Send-panel shape: choice 0 = recipient row (above the grid), choices
    /// 1..=8 = the 2x4 slot grid, choice 9 = Cancel (below the grid).
    fn send_panel_grid() -> kuluu_snapshot::DialogGrid {
        kuluu_snapshot::DialogGrid {
            cols: 4,
            rows: 2,
            cells: (0..8u32)
                .map(|i| kuluu_snapshot::DialogGridCell {
                    choice: Some(i + 1),
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn grid_nav_walks_cells_spatially() {
        let g = send_panel_grid();
        // Right along the top row; clamped at the edge.
        assert_eq!(grid_nav_choice(&g, 9, 1, 1, 0), 2);
        assert_eq!(grid_nav_choice(&g, 9, 4, 1, 0), 4);
        // Down keeps the column; up returns.
        assert_eq!(grid_nav_choice(&g, 9, 2, 0, 1), 6);
        assert_eq!(grid_nav_choice(&g, 9, 6, 0, -1), 2);
    }

    #[test]
    fn grid_nav_bridges_flat_rows_above_and_below() {
        let g = send_panel_grid();
        // Up off the top row lands on the recipient row (choice 0)…
        assert_eq!(grid_nav_choice(&g, 9, 3, 0, -1), 0);
        // …and down from it re-enters the grid.
        assert_eq!(grid_nav_choice(&g, 9, 0, 0, 1), 1);
        // Down off the bottom row lands on Cancel (choice 9)…
        assert_eq!(grid_nav_choice(&g, 9, 7, 0, 1), 9);
        // …and up from Cancel re-enters the grid's bottom row.
        assert_eq!(grid_nav_choice(&g, 9, 9, 0, -1), 5);
        // Left/right on flat rows do nothing.
        assert_eq!(grid_nav_choice(&g, 9, 0, 1, 0), 0);
        assert_eq!(grid_nav_choice(&g, 9, 9, -1, 0), 9);
    }

    #[test]
    fn grid_nav_skips_inert_cells() {
        // Incoming-box shape: only slots 0 and 6 occupied, no flat rows
        // besides the trailing Cancel (choice 2).
        let mut g = send_panel_grid();
        for (i, cell) in g.cells.iter_mut().enumerate() {
            cell.choice = match i {
                0 => Some(0),
                6 => Some(1),
                _ => None,
            };
        }
        // Down from (0,0) reaches (2,1) — nearest selectable on the next row.
        assert_eq!(grid_nav_choice(&g, 2, 0, 0, 1), 1);
        // Right from (0,0) has no selectable neighbor on that row.
        assert_eq!(grid_nav_choice(&g, 2, 0, 1, 0), 0);
        // Down off the bottom row hits Cancel; up from Cancel returns.
        assert_eq!(grid_nav_choice(&g, 2, 1, 0, 1), 2);
        assert_eq!(grid_nav_choice(&g, 2, 2, 0, -1), 1);
    }

    #[test]
    fn self_only_actions_skip_sub_target() {
        use kuluu_render::hud::menu::DynamicMenuAction as A;
        use kuluu_render::input_mode::SubTargetAction as S;
        // Boost (ability 39, validTarget SELF) casts on <me> — no <st> prompt.
        assert_eq!(
            sub_target_action_for(A::JobAbility { ability_id: 39 }),
            None
        );
        // Provoke (ability 35, ENEMY) still opens the sub-target cursor.
        assert_eq!(
            sub_target_action_for(A::JobAbility { ability_id: 35 }),
            Some(S::Ability(35))
        );
        // Cure (spell 1, PARTY) still prompts.
        assert_eq!(
            sub_target_action_for(A::CastSpell { spell_id: 1 }),
            Some(S::Spell(1))
        );
    }
}
