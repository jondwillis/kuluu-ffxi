use super::*;
use kuluu_render::ParityValue;

fn minimap_retail_desc(state: &kuluu_render::minimap::MinimapState, zone: Option<u16>) -> String {
    use kuluu_render::minimap::RetailStatus;
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
    // The file id is not named here: the loader picks it out of the FFXiMain
    // zone-map record, and quoting the POLUtils table's id instead would report
    // a different map than the one being loaded (kuluu-bqm5).
    format!(
        "pending (zone {z}; img={} rzone={:?} failed={})",
        state.retail_image.is_some(),
        state.retail_zone,
        state.retail_failed_zones.contains(&z),
    )
}

/// The one applied Current Time outcome (retail runner law, vault main_menu.md §12): toggles the
/// widget and prints both time lines. Shared by the menu row's TextCommand seam and typed `/clock`.
pub(super) fn activate_current_time(
    vana_clock: &kuluu_render::vana_time::VanaClock,
    vana_clock_visible: &mut kuluu_render::hud::vana_clock::VanaClockVisible,
    scene_state: &mut SceneState,
) {
    vana_clock_visible.0 = !vana_clock_visible.0;
    for line in kuluu_render::hud::vana_clock::current_time_chat_lines(vana_clock) {
        push_system_chat_line(scene_state, line);
    }
}

/// The applied `SetVanaClock` outcome (bead kuluu-o9qt7.28): the shared primitive behind both
/// `apply_slash_outcome`'s arm and the menu rows' seam, so typed `/clock [show|hide|toggle]`
/// and the Current Time row land identical state and chat lines.
pub(super) fn set_current_time(
    setting: Option<bool>,
    vana_clock: &kuluu_render::vana_time::VanaClock,
    vana_clock_visible: &mut kuluu_render::hud::vana_clock::VanaClockVisible,
    scene_state: &mut SceneState,
) {
    if let Some(next) = setting {
        vana_clock_visible.0 = next;
        for line in kuluu_render::hud::vana_clock::current_time_chat_lines(vana_clock) {
            push_system_chat_line(scene_state, line);
        }
    } else {
        activate_current_time(vana_clock, vana_clock_visible, scene_state);
    }
}

/// The chat echo `apply_slash_outcome`'s OpenMenu arm prints for a menu kind; the menu rows'
/// seam (bead kuluu-o9qt7.28) echoes through this same table so both surfaces read alike.
pub(super) fn menu_open_label(kind: MenuKind) -> std::borrow::Cow<'static, str> {
    match kind {
        kuluu_render::MenuKind::Magic => "Magic".into(),
        // The picker keeps the Magic caption; a filtered list echoes its school
        // (vault main_menu.md §21 numbers).
        kuluu_render::MenuKind::MagicGroups => "Magic".into(),
        kuluu_render::MenuKind::MagicGroup(school) => format!("MagicList({school})").into(),
        kuluu_render::MenuKind::Abilities => "Abilities".into(),
        // Same pair law for the abimenu picker and its filtered list
        // (vault main_menu.md §23).
        kuluu_render::MenuKind::AbilitiesGroups => "Abilities".into(),
        kuluu_render::MenuKind::AbilitiesGroup(group) => format!("AbilityList({group})").into(),
        kuluu_render::MenuKind::Items => "Items".into(),
        // ur/150: the bucket hub echoes `Key Items`; a bucket list echoes its
        // own name, like MagicGroup echoes its school above.
        kuluu_render::MenuKind::KeyItems => "Key Items".into(),
        kuluu_render::MenuKind::KeyItemList { category } => {
            kuluu_render::hud::menu::KEY_ITEM_CATEGORY_LABELS
                .get(category as usize)
                .copied()
                .unwrap_or("Key Items")
                .to_string()
                .into()
        }
        // ur/152: Examine and the description window echo the item's own name,
        // like a bucket list echoes its category above.
        kuluu_render::MenuKind::KeyItemExamine { id }
        | kuluu_render::MenuKind::KeyItemText { id } => {
            kuluu_render::hud::menu::key_item_row_label(id).into()
        }
        kuluu_render::MenuKind::UsableItems => "Items (usable)".into(),
        kuluu_render::MenuKind::Equipment => "Equipment".into(),
        kuluu_render::MenuKind::Root => "Root".into(),
        kuluu_render::MenuKind::Config => "Config".into(),
        kuluu_render::MenuKind::Controls => "Controls".into(),
        kuluu_render::MenuKind::Debug => "Debug".into(),
        kuluu_render::MenuKind::Graphics => "Graphics".into(),
        kuluu_render::MenuKind::GraphicsDlss => "DLSS Config".into(),
        kuluu_render::MenuKind::Status => "Status".into(),

        kuluu_render::MenuKind::Communication => "Communication".into(),
        kuluu_render::MenuKind::EmoteList => "Emote List".into(),
        // Echoes the retail window name (`menu    abisortw`, registry 256): no DAT title
        // exists [V-negative] and kuluu never opens it as a focused level on its own —
        // it rides below an EmoteList push (bead kuluu-o9qt7.34).
        kuluu_render::MenuKind::AbilitySort { .. } => "abisortw".into(),
        // Page-1 row 10's window (vault main_menu.md §27): retail opens `menu    link5`,
        // which is the same surface a `/menu linkshell` style entry reaches here.
        kuluu_render::MenuKind::Linkshell => "Link Shell".into(),
        kuluu_render::MenuKind::SynthLauncher => "Synthesis".into(),
        kuluu_render::MenuKind::CraftList => "Craft List".into(),
        kuluu_render::MenuKind::RegionInfo { .. } => "Region Info".into(),
        kuluu_render::MenuKind::MissionList => "Missions".into(),
        kuluu_render::MenuKind::MissionDetail { log, .. } => format!("MissionLog({log})").into(),
        // The ur/80 text drill echoes its payload like the quest01 detail does.
        kuluu_render::MenuKind::MissionText { log, id } => {
            format!("MissionText({log}#{id})").into()
        }
        // Bazaar row's stall picker (vault main_menu.md §33).
        kuluu_render::MenuKind::BazaarStalls => "Bazaar".into(),
        // …and the second row of that panel, which opens the comment field over the chat log.
        kuluu_render::MenuKind::BazaarComment => "Edit Comment".into(),
        kuluu_render::MenuKind::MacrosPalette => "Macros".into(),
        kuluu_render::MenuKind::RetailConfig => "Config".into(),
        // A config page window inherits the hub's caption (vault main_menu.md §47).
        kuluu_render::MenuKind::RetailConfigPage(_) => "Config".into(),
        // …and so does a config sub-window (vault main_menu.md §47).
        kuluu_render::MenuKind::RetailConfigSub(_) => "Config".into(),
        kuluu_render::MenuKind::HelpDeskPanel => "Help Desk".into(),
        // A statcom2 companion inherits the Status caption (its retail window
        // titles come from their own sheet, not a menu title token; vault
        // main_menu.md §59).
        kuluu_render::MenuKind::StatusDetail { .. } => "Status".into(),
        // The Unity and alter ego point windows echo the Status row that opens them.
        kind @ (kuluu_render::MenuKind::UnityMenu
        | kuluu_render::MenuKind::UnityChat
        | kuluu_render::MenuKind::UnityRankingChooser
        | kuluu_render::MenuKind::UnityRanking { .. }
        | kuluu_render::MenuKind::UnityLeaders
        | kuluu_render::MenuKind::AlterEgoCategories
        | kuluu_render::MenuKind::AlterEgoUpgrades { .. }
        | kuluu_render::MenuKind::AlterEgoConfirm { .. }
        | kuluu_render::MenuKind::Merit(_)
        | kuluu_render::MenuKind::Arrange(_)) => kuluu_render::hud::menu::menu_title(kind).into(),
        // commenu Friend List widget (vault main_menu.md §37).
        kuluu_render::MenuKind::FriendList => "Friend List".into(),
        // Equipment-row variants (vault main_menu.md §39): pupequip
        // keeps the parent caption; mp_mncst is catalog-titled.
        kuluu_render::MenuKind::AutomatonEquip => "Equipment".into(),
        kuluu_render::MenuKind::MpCustomize => "Customization".into(),
        // The Map-row myroom variant inherits its parent caption (the
        // `myro` DAT record has no title token; vault main_menu.md §40).
        kuluu_render::MenuKind::HomeView => "Map".into(),
        // `mogcont`'s own DAT title token is the View House row caption (catalog 107; §32), so the
        // box list wears that string as well.
        kuluu_render::MenuKind::MogContainers => kuluu_render::hud::menu::ROOT_VIEW_HOUSE.into(),
        // …and the roomlist pair likewise inherits the Remodel row's caption
        // (`roomlist` carries no title token — widget captions are −1 [V-neg]; bead
        // kuluu-o9qt7.13.5, vault main_menu.md §59 addendum 20/21).
        kuluu_render::MenuKind::RoomList | kuluu_render::MenuKind::RoomListConfirm { .. } => {
            "Remodel".into()
        }
        // The Change Jobs drill family echoes its DAT caption (window 'jobchang',
        // widgets catalog-titled 65/66; vault main_menu.md §53).
        kuluu_render::MenuKind::JobChange | kuluu_render::MenuKind::JobPick { .. } => {
            "Change Jobs".into()
        }
        // mcrselec/mcres20 and the line editor likewise inherit `Macros`
        // (§41 title law).
        kuluu_render::MenuKind::MacroSelect
        | kuluu_render::MenuKind::MacroEditor { .. }
        | kuluu_render::MenuKind::MacroResult => "Macros".into(),
        // …while the book options window echoes its own band word, and a pallet names its book.
        kuluu_render::MenuKind::MacroBookOptions { .. }
        | kuluu_render::MenuKind::EquipSetOptions { .. } => "Options".into(),
        kuluu_render::MenuKind::MacroPallet { book } => {
            format!("Macro Set Book{:02} (#{})", book + 1, book + 1).into()
        }
        // The /quest seam (vault main_menu.md §31; ur/87 hub first).
        kuluu_render::MenuKind::QuestMenu | kuluu_render::MenuKind::QuestList { .. } => {
            "Quests".into()
        }
        kuluu_render::MenuKind::QuestDetail { log, id } => format!("QuestLog({log},{id})").into(),

        kuluu_render::MenuKind::ItemAction { item_no, .. } => {
            format!("ItemAction({item_no})").into()
        }
        kuluu_render::MenuKind::ItemDropConfirm { item_no, .. } => {
            format!("ItemDropConfirm({item_no})").into()
        }
        kuluu_render::MenuKind::EquipSlot(slot) => format!("EquipSlot({slot})").into(),
        kuluu_render::MenuKind::Map => "Map".into(),
        // One surface for the prtyN family (vault main_menu.md section 24).
        kuluu_render::MenuKind::Party => "Party".into(),
        // One surface for the trade/gift pair and its bag picker
        // (vault main_menu.md section 25).
        kuluu_render::MenuKind::Trade => "Trade".into(),
        kuluu_render::MenuKind::TradePick { trade_index } => {
            format!("TradePick({trade_index})").into()
        }
        // One surface for the searchma/scresult family
        // (vault main_menu.md section 26).
        kuluu_render::MenuKind::Search => "Search".into(),
        kuluu_render::MenuKind::PowerPrompt { shut_down } => if shut_down {
            "ShutDownPrompt"
        } else {
            "LogOutPrompt"
        }
        .into(),
        // The Config family's reset dialog echoes the family caption (its retail body
        // text is a DAT string, catalog table 0xb id 0x8F; vault main_menu.md §59
        // addenda 7–8).
        kuluu_render::MenuKind::KeyAssignResetPrompt => "Config".into(),
    }
}

/// Why `/layout` would be refused right now: the session's own refusal chain over the snapshot's
/// view of the house (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md B1).
pub(super) fn layout_refusal(
    snap: &kuluu_snapshot::SceneSnapshot,
) -> Option<kuluu_session::myroom_layout::Refusal> {
    use kuluu_session::myroom_layout as layout;
    let house = snap.myroom.map(|room| layout::HouseView {
        map_number: room.model,
        sub_map: room.sub_map,
        door_open: room.door_open,
    });
    let gates = layout::ArrangeGates::for_house(
        snap.myroom_mode,
        house,
        ffxi_proto::decode::ServerLoginMyroom::login_in_mog_house(snap.login_state),
    );
    layout::plan_arrange(
        layout::ArrangeRequest::from_slash(layout::MODE_LAYOUT),
        &gates,
        None,
    )
    .err()
}

/// `/layout`, typed or run by the myroom Layout row, refuses on the client: it prints the client
/// message or plays the buzzer and nothing goes to the session (which runs the same chain before the
/// wire). Returns whether `command` was a refused `/layout`.
pub(super) fn layout_refused(
    command: &AgentCommand,
    scene_state: &mut SceneState,
    dynamic: &mut kuluu_render::hud::menu::DynamicMenu,
) -> bool {
    use kuluu_session::myroom_layout::{RefusalSignal, MODE_LAYOUT};
    if !matches!(command, AgentCommand::MyroomMode { mode } if *mode == MODE_LAYOUT) {
        return false;
    }
    let Some(refusal) = layout_refusal(&scene_state.snapshot) else {
        return false;
    };
    match refusal.signal() {
        RefusalSignal::Buzzer => dynamic
            .ui_sounds
            .push(kuluu_render::hud::menu::UiSound::Buzzer),
        RefusalSignal::Message(id) => {
            let line = dynamic.arrange.texts.client_message(id);
            push_system_chat_line(scene_state, line);
        }
        RefusalSignal::Silent => {}
    }
    true
}

/// The single text-command runner for the Commands-menu rows (bead kuluu-o9qt7.28): a row's
/// literal goes through `parse_slash` — the one classifier/alias layer typed input uses
/// (FFXiMain.dll retail-2026-09 RVA 0x80DC0, vault main_menu.md §12) — and the outcome is
/// applied through this function's subset of `apply_slash_outcome`'s arms. The parse arguments
/// all ride the snapshot; the fishing gate comes from `gate_from_snapshot`, so keyboard rows,
/// mouse clicks and typed input resolve identically without caller-side plumbing.
///
/// The subset covers every outcome the three row literals can produce (Current Time →
/// `SetVanaClock`, Key Items/Quests → `OpenMenu`), plus `Command(s)` pass-through and
/// `SystemMessage`; the myroom Layout row's `/layout` refuses here as typed ([`layout_refused`]).
/// Two typed-only apply effects are intentionally absent here: the heal/rest
/// stance mirror and the enhanced shutdown-counter writer, both driven by `/logout`-family rows
/// that answer through PowerPrompt instead of this seam.
pub(super) fn apply_menu_text_command(
    cmd: &str,
    command_surface: &crate::view_native::command_surface::CommandSurface,
    stack: &mut MenuStack,
    vana_clock: &kuluu_render::vana_time::VanaClock,
    vana_clock_visible: &mut kuluu_render::hud::vana_clock::VanaClockVisible,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    self_pos: kuluu_snapshot::Vec3,
    target_id: Option<u32>,
    dynamic: &mut kuluu_render::hud::menu::DynamicMenu,
) {
    let battle_target = match scene_state.snapshot.current_goal {
        Some(kuluu_snapshot::ReactorGoal::Engaged { target_id, .. }) => Some(target_id),
        _ => None,
    };
    let outcome = parse_slash(
        cmd,
        command_surface,
        &scene_state.snapshot.entities,
        self_pos,
        target_id,
        scene_state.snapshot.zone_id,
        scene_state.snapshot.self_char_id,
        &scene_state.snapshot.party,
        kuluu_render::fishing_spot::gate_from_snapshot(&scene_state.snapshot),
        battle_target,
        scene_state.snapshot.self_pet_targid,
    );
    match outcome {
        SlashOutcome::SetVanaClock(setting) => {
            set_current_time(setting, vana_clock, vana_clock_visible, scene_state);
        }
        SlashOutcome::OpenMenu(kind) => {
            let label = menu_open_label(kind);
            push_system_chat_line(scene_state, format!("[menu] opened {label}"));
            stack.push(kind);
        }
        SlashOutcome::SystemMessage(text) => {
            for line in text.split('\n') {
                push_system_chat_line(scene_state, line.to_string());
            }
        }
        SlashOutcome::Command(command) => {
            if layout_refused(&command, scene_state, dynamic) {
                return;
            }
            if let Some(toast) = reqlogout_ack_text(&command) {
                push_system_chat_line(scene_state, toast.into());
            }
            if let Err(e) = cmd_tx.try_send(command) {
                push_system_chat_line(scene_state, format!("command dropped (channel issue): {e}"));
            }
        }
        SlashOutcome::Commands(commands) => {
            for command in commands {
                if let Some(toast) = reqlogout_ack_text(&command) {
                    push_system_chat_line(scene_state, toast.into());
                }
                if let Err(e) = cmd_tx.try_send(command) {
                    push_system_chat_line(
                        scene_state,
                        format!("command dropped (channel issue): {e}"),
                    );
                }
            }
        }
        _ => {
            // No row literal reaches these today; keep the old shim's refusal line for any
            // future row whose command needs an apply effect this seam does not own.
            push_system_chat_line(scene_state, format!("[menu] {cmd} — not implemented"));
        }
    }
}

/// Apply a parsed slash-command outcome. Disengage (/disengage, /cancel)
/// releases the camera lock like the H toggle; the lock is a client-side
/// latch the wire cancel does not touch. Quit sends ReqLogout and Disconnect
/// over the one command channel; the session transmits before it reads again,
/// so the server learns we are leaving rather than seeing a dropped link.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_slash_outcome(
    outcome: SlashOutcome,
    target: &mut Target,
    cmd_tx: &Sender<AgentCommand>,
    scene_state: &mut SceneState,
    exit: &mut MessageWriter<AppExit>,
    navmesh_visible: &mut crate::view_native::navmesh_overlay::NavmeshOverlayVisible,
    navmesh_state: &crate::view_native::navmesh_overlay::NavmeshState,
    self_pos: kuluu_snapshot::Vec3,
    bindings: &mut Bindings,
    keybinds_state: &mut KeybindsStateRes,
    #[cfg(unix)] agent_paused: Option<&crate::view_native::AgentPaused>,
    session_event_tx: Option<&crate::view_native::SessionEventTx>,
    slash_writers: &mut SlashWriters,
    dynamic_menu: &mut kuluu_render::hud::menu::DynamicMenu,
    draw_distance: &mut kuluu_render::dat_mzb::DrawDistance,
) {
    #[cfg(not(unix))]
    let _ = session_event_tx;
    match outcome {
        SlashOutcome::Command(cmd) => {
            if layout_refused(&cmd, scene_state, dynamic_menu) {
                return;
            }
            if let Some(toast) = reqlogout_ack_text(&cmd) {
                push_system_chat_line(scene_state, toast.into());
            }
            #[cfg(feature = "enhanced-shutdown-counter")]
            if let Some(shutdown) = reqlogout_starts_countdown(&cmd) {
                slash_writers
                    .logout_requested
                    .write(kuluu_render::hud::logout_countdown::LogoutRequested { shutdown });
            }
            mirror_heal_stance(&cmd, &mut slash_writers.rest_stance);
            if matches!(cmd, AgentCommand::Cancel) {
                slash_writers.lock_on.target_id = None;
            }
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
                #[cfg(feature = "enhanced-shutdown-counter")]
                if let Some(shutdown) = reqlogout_starts_countdown(&cmd) {
                    slash_writers
                        .logout_requested
                        .write(kuluu_render::hud::logout_countdown::LogoutRequested { shutdown });
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
            let _ = cmd_tx.try_send(AgentCommand::ReqLogout {
                kind: kuluu_session::state::ReqLogoutKind::ShutdownOn,
            });
            let _ = cmd_tx.try_send(AgentCommand::Disconnect);
            exit.write_default();
            crate::view_native::exit_watchdog::arm();
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
        // The client-side config-option store write (`/furcc`, `/layoutctrl`). Retail's
        // setters (.text FFXiMain.dll retail-2026-09 `.text 0x1938F0` for key 6, `.text
        // 0x193830` for key 0xC1) are plain stores — no packet, no cache flush — and each
        // setter arm immediately echoes through the chat emitter (`.text 0x129DE0`) with a
        // GetText table-9 id; kuluu renders that id from DAT via `client_message`.
        SlashOutcome::ConfigSet {
            store_id,
            value,
            message_id,
        } => {
            dynamic_menu.config_option_values.insert(store_id, value);
            let line = dynamic_menu.client_message(slash_writers.dat_root.0.as_deref(), message_id);
            push_system_chat_line(scene_state, line);
        }
        // …and the query arm both handlers fall through to for a bare or unrecognized
        // word (store getters `.text 0x193890`/`.text 0x193850`; `test al,al` picks the
        // non-zero/zero message — bead kuluu-o9qt7.41). Absent keys read as zero.
        SlashOutcome::ConfigQuery {
            store_id,
            nonzero_message_id,
            zero_message_id,
            write_back_normalized,
        } => {
            // `/statusparty`'s merge point (.text FFXiMain.dll retail-2026-09 0x7EB1C) re-runs
            // the setter with the bool-normalized value; `config_query` carries that law plus
            // the plain-read getters for furcc/layoutctrl.
            let current = dynamic_menu.config_query(store_id, write_back_normalized);
            let message_id = if current != 0 {
                nonzero_message_id
            } else {
                zero_message_id
            };
            let line = dynamic_menu.client_message(slash_writers.dat_root.0.as_deref(), message_id);
            push_system_chat_line(scene_state, line);
        }
        SlashOutcome::SetSitStance(toggle) => {
            use crate::view_native::slash_commands::SitToggle;
            use kuluu_render::combat_stance::RestKind;
            let next = match toggle {
                SitToggle::On => RestKind::Sit,
                SitToggle::Off => RestKind::None,
                SitToggle::Toggle => match slash_writers.rest_stance.kind {
                    RestKind::Sit => RestKind::None,

                    RestKind::Heal => {
                        let _ = cmd_tx.try_send(AgentCommand::Heal {
                            mode: kuluu_session::state::HealMode::Off,
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
            let bevy_pos = kuluu_render::ffxi_to_bevy(world_pos);
            slash_writers.load_mmb.write(LoadMmbRequest {
                light_bindings: Default::default(),
                area_id: 0,
                file_id,
                chunk_idx,
                world_pos: bevy_pos,
                entity_id,
                world_transform: None,
                water: None,
                lod: None,
                door: None,
                slot: kuluu_render::dat_mzb::ZONE_SLOT_MAIN,
                sub_area_link: 0,
                voyage_backdrop: false,
                static_model: None,
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
            let resolved = path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(crate::view_native::screenshot::next_default_path);
            slash_writers
                .screenshot
                .write(crate::view_native::screenshot::ScreenshotRequest { path: resolved });
        }
        SlashOutcome::PlayBgm { track_id } => {
            slash_writers
                .event_log
                .recent
                .push_back(kuluu_snapshot::ViewerEvent::MusicChanged { slot: 0, track_id });
            push_system_chat_line(scene_state, format!("/bgm {track_id}: queued"));
        }
        SlashOutcome::PlaySfx { se_id } => {
            slash_writers
                .sfx_event
                .write(kuluu_render::audio::SfxEvent::new(se_id));
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
        SlashOutcome::SetDebugChat(setting) => {
            let next = setting.unwrap_or(!slash_writers.graphics.debug_chat);
            slash_writers.graphics.debug_chat = next;
            push_system_chat_line(
                scene_state,
                format!("/debugchat: {}", if next { "on" } else { "off" }),
            );
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
        SlashOutcome::SetNoClip(setting) => {
            let next = setting.unwrap_or(!slash_writers.hud_panels.noclip);
            slash_writers.hud_panels.noclip = next;
            push_system_chat_line(
                scene_state,
                format!(
                    "/noclip: {} (wall collision {})",
                    if next { "on" } else { "off" },
                    if next { "bypassed" } else { "active" }
                ),
            );
        }
        SlashOutcome::SetAutoAttack(setting) => {
            let next = setting.unwrap_or(!slash_writers.auto_attack.enabled);
            slash_writers.auto_attack.enabled = next;
            push_system_chat_line(
                scene_state,
                format!("/autoattack: {}", if next { "on" } else { "off" }),
            );
        }
        SlashOutcome::SetVanaClock(setting) => {
            // The single applied clock outcome (bead kuluu-o9qt7.28): the widget flips and both
            // retail time lines print, whether typed or driven by the Current Time menu row.
            set_current_time(
                setting,
                &slash_writers.vana_clock,
                &mut slash_writers.vana_clock_visible,
                scene_state,
            );
        }
        SlashOutcome::SetRenderScale(setting) => {
            let g = &mut *slash_writers.graphics;
            if let Some(v) = setting {
                g.render_scale = v.clamp(0.25, 2.0);
                g.preset = kuluu_render::QualityPreset::Custom;
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
        SlashOutcome::SetZoneLines(op) => {
            use crate::view_native::slash_commands::ZoneLineOp;
            use kuluu_render::ZoneLineDisplay;

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
            use crate::view_native::slash_commands::LightsOp;
            use kuluu_render::graphics_settings::{DynamicLights, SHADOWED_LIGHTS_SLOTS};

            let g = &mut *slash_writers.graphics;
            let chat = match op {
                LightsOp::Status => format!(
                    "/lights: {} · shadowed {} · flicker {}",
                    g.dynamic_lights.label(),
                    g.shadowed_lights,
                    if g.light_flicker { "on" } else { "off" },
                ),
                LightsOp::Enable(v) => {
                    let on = v.unwrap_or(!g.dynamic_lights.point_shadows_enabled());
                    g.dynamic_lights = if on {
                        DynamicLights::Enhanced
                    } else {
                        DynamicLights::Off
                    };
                    format!("/lights: {}", g.dynamic_lights.label())
                }
                LightsOp::Shadowed(v) => {
                    let v = v.min(*SHADOWED_LIGHTS_SLOTS.last().unwrap_or(&0));
                    g.shadowed_lights = v;
                    format!("/lights shadowed: {v}")
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
            use crate::view_native::slash_commands::MinimapOp;
            use kuluu_render::minimap::MinimapMode;
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
                    let half = kuluu_render::minimap::zone_half_span(
                        slash_writers
                            .minimap_state
                            .active_aabb(*slash_writers.minimap_mode),
                    );
                    slash_writers
                        .minimap_zoom
                        .zoom_by(1.0 / kuluu_render::minimap::ZOOM_STEP_FACTOR, half);
                    slash_writers.minimap_view.idle_frames = 0;
                    format_zoom_status(&slash_writers.minimap_zoom)
                }
                MinimapOp::ZoomOut => {
                    let half = kuluu_render::minimap::zone_half_span(
                        slash_writers
                            .minimap_state
                            .active_aabb(*slash_writers.minimap_mode),
                    );
                    slash_writers
                        .minimap_zoom
                        .zoom_by(kuluu_render::minimap::ZOOM_STEP_FACTOR, half);
                    slash_writers.minimap_view.idle_frames = 0;
                    format_zoom_status(&slash_writers.minimap_zoom)
                }
                MinimapOp::ZoomFit => {
                    slash_writers.minimap_zoom.radius_yalms = None;
                    slash_writers.minimap_view.idle_frames = 0;
                    "/minimap zoom: fit-to-zone".into()
                }
                MinimapOp::ZoomSet(r) => {
                    let clamped = r.max(kuluu_render::minimap::ZOOM_MIN_RADIUS);
                    slash_writers.minimap_zoom.radius_yalms = Some(clamped);
                    slash_writers.minimap_view.idle_frames = 0;
                    format!("/minimap zoom: radius={clamped:.0} yalms")
                }
                MinimapOp::ZoomReset => {
                    *slash_writers.minimap_zoom = kuluu_render::minimap::MinimapZoom::default();
                    slash_writers.minimap_view.pan_offset_xz = bevy::math::Vec2::ZERO;
                    slash_writers.minimap_view.idle_frames = 0;
                    "/minimap zoom: reset to defaults".into()
                }
            };
            push_system_chat_line(scene_state, chat);
        }
        SlashOutcome::ActorDiag { use_target } => {
            let id = if use_target {
                target.id
            } else {
                scene_state.snapshot.self_char_id
            };
            let lines = match id {
                None if use_target => vec!["/actordiag: no target selected".to_string()],
                None => vec!["/actordiag: self id unknown (not in game yet?)".to_string()],
                Some(id) => match scene_state.snapshot.entities.iter().find(|e| e.id == id) {
                    None => vec![format!("/actordiag: entity {id} not in the snapshot")],
                    Some(e) => match &e.look {
                        None => vec![format!("/actordiag: entity {id} carries no look data")],
                        Some(look) => match slash_writers.dat_root.0.as_deref() {
                            Some(root) => kuluu_render::actor_diag::report(
                                root,
                                id,
                                e.name.as_deref().unwrap_or("?"),
                                look,
                            ),
                            None => vec!["/actordiag: no DAT install loaded".to_string()],
                        },
                    },
                },
            };
            for line in lines {
                push_system_chat_line(scene_state, line);
            }
        }
        SlashOutcome::Overlay(op) => {
            use crate::view_native::slash_commands::OverlayOp;
            let chat = match slash_writers.dat_root.0.as_ref() {
                None => "/overlay: no DAT install loaded".to_string(),
                Some(root) => {
                    let active = root.overlays();
                    let store = slash_writers.overlay_store.as_ref().map(|r| &r.store);
                    // `None` from the store means no override file, so the
                    // active list is whatever discovery found.
                    let overridden = store.and_then(|s| s.load().ok().flatten()).is_some();
                    let mut next: Option<Vec<std::path::PathBuf>> = None;
                    let mut reset = false;
                    let mut msg = match &op {
                        OverlayOp::List => {
                            let source = if overridden { "override" } else { "discovered" };
                            if active.is_empty() {
                                format!("/overlay: none active ({source})")
                            } else {
                                let list = active
                                    .iter()
                                    .enumerate()
                                    .map(|(i, p)| format!("  {}. {}", i + 1, p.display()))
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                format!("/overlay: {} active ({source})\n{list}", active.len())
                            }
                        }
                        OverlayOp::Add(dir) => {
                            if !dir.is_dir() {
                                format!("/overlay add: not a directory: {}", dir.display())
                            } else {
                                let mut v = active.clone();
                                v.push(dir.clone());
                                let n = v.len();
                                next = Some(v);
                                format!("/overlay: added {} ({n} active)", dir.display())
                            }
                        }
                        OverlayOp::Remove(n) => match active.get(n - 1) {
                            None => {
                                format!("/overlay remove: no entry {n} (have {})", active.len())
                            }
                            Some(gone) => {
                                let gone = gone.display().to_string();
                                let mut v = active.clone();
                                v.remove(n - 1);
                                next = Some(v);
                                format!("/overlay: removed {gone}")
                            }
                        },
                        OverlayOp::Clear => {
                            next = Some(Vec::new());
                            "/overlay: cleared — the install's own DATs only".to_string()
                        }
                        OverlayOp::Reset => {
                            reset = true;
                            let found = ffxi_dat::archive::discover_overlays(root.root());
                            let n = found.len();
                            root.set_overlays(found);
                            format!("/overlay: back to discovery ({n} found)")
                        }
                    };

                    if let Some(v) = next {
                        root.set_overlays(v.clone());
                        match store {
                            Some(s) => {
                                if let Err(e) = s.save(&v) {
                                    msg.push_str(&format!("\n  (not saved: {e})"));
                                }
                            }
                            None => msg.push_str("\n  (not saved: no config dir)"),
                        }
                    }
                    if reset {
                        match store {
                            Some(s) => {
                                if let Err(e) = s.clear() {
                                    msg.push_str(&format!("\n  (override not removed: {e})"));
                                }
                            }
                            None => msg.push_str("\n  (no override file to remove)"),
                        }
                    }
                    if !matches!(op, OverlayOp::List) {
                        // Only later reads go through the new path; anything
                        // already decoded into an asset keeps the old bytes.
                        msg.push_str(
                            "\n  (already-loaded DAT assets keep the old bytes until reload)",
                        );
                    }
                    msg
                }
            };
            push_system_chat_line(scene_state, chat);
        }
        SlashOutcome::SetSound(op) => {
            use crate::view_native::slash_commands::SoundOp;
            let mute = &mut *slash_writers.audio_mute;

            let apply = |cur: &mut bool, target: Option<bool>| {
                *cur = target.unwrap_or(!*cur);
            };
            let chat = match op {
                SoundOp::SetBgm(target) => {
                    apply(&mut mute.bgm, target);
                    format!("/mutebgm: {}", if mute.bgm { "on" } else { "off" })
                }
                SoundOp::SetSfx(target) => {
                    apply(&mut mute.sfx, target);
                    format!("/mutese: {}", if mute.sfx { "on" } else { "off" })
                }
            };
            push_system_chat_line(scene_state, chat);
        }
        SlashOutcome::SetDrawDistance(op) => {
            use crate::view_native::slash_commands::DrawDistanceOp;
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
            let bevy_pos = kuluu_render::ffxi_to_bevy(world_pos);
            slash_writers.load_mzb.write(LoadMzbRequest {
                file_id,
                chunk_idx,
                world_pos: bevy_pos,

                auto_loaded: false,
                slot: kuluu_render::dat_mzb::ZONE_SLOT_MAIN,
                active_sub_area: None,
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
        SlashOutcome::SubArea { op, self_pos } => {
            apply_sub_area(
                op,
                self_pos,
                scene_state,
                slash_writers.dat_root.0.as_deref(),
                &mut slash_writers.set_sub_area,
            );
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
        #[cfg(debug_assertions)]
        SlashOutcome::Widescan => {
            let _ = cmd_tx.try_send(AgentCommand::WidescanRequest);
            let rows = kuluu_render::hud::map_screen::widescan_rows(&scene_state.snapshot);
            if rows.is_empty() {
                push_system_chat_line(
                    scene_state,
                    "[widescan] no targets (requesting refresh…)".into(),
                );
            } else {
                push_system_chat_line(scene_state, format!("[widescan] {} target(s):", rows.len()));
                for row in rows {
                    push_system_chat_line(scene_state, format!("  {}", row.label));
                }
            }
        }
        SlashOutcome::OpenMenu(kind) => {
            let label = menu_open_label(kind);
            push_system_chat_line(scene_state, format!("[menu] opened {label}"));
        }
        SlashOutcome::OpenSubTarget { .. } => {
            // The caller's mode override opens the sub-target cursor; there is
            // no command to apply until it is confirmed.
        }
    }
}

fn apply_sub_area(
    op: SubAreaOp,
    self_pos: kuluu_snapshot::Vec3,
    scene_state: &mut SceneState,
    root: Option<&ffxi_dat::DatRoot>,
    set_sub_area: &mut MessageWriter<kuluu_render::sub_area_activation::SetSubArea>,
) {
    let Some(root) = root else {
        push_system_chat_line(scene_state, "/subarea: no DAT install loaded".into());
        return;
    };
    let Some(zone_file_id) = kuluu_render::snapshot::effective_zone_file_id(&scene_state.snapshot)
    else {
        push_system_chat_line(
            scene_state,
            "/subarea: no zone DAT for the current zone".into(),
        );
        return;
    };
    let subs = match kuluu_render::dat_mzb::zone_sub_areas(root, zone_file_id) {
        Ok(s) => s,
        Err(e) => {
            push_system_chat_line(scene_state, format!("/subarea: {e}"));
            return;
        }
    };
    if subs.is_empty() {
        push_system_chat_line(
            scene_state,
            format!("/subarea: zone DAT {zone_file_id} declares no sub-areas"),
        );
        return;
    }

    // Snapshot order is (x, ground depth, height); the RID rects keep the DAT's
    // (x, height, ground depth). Same remap as `reactor::is_inside_dat_obb`.
    let here = [self_pos.x, self_pos.z, self_pos.y];
    let wanted = match op {
        SubAreaOp::List => {
            let mut lines = vec![format!(
                "/subarea: zone DAT {zone_file_id} declares {} interior(s)",
                subs.len()
            )];
            for s in &subs {
                lines.push(format!(
                    "  {:#x} -> DAT {}{} · {} trigger(s){}",
                    s.sub_area.id,
                    s.sub_area.file_id,
                    if s.resolves { "" } else { " (MISSING)" },
                    s.sub_area.triggers.len(),
                    if s.sub_area.contains(here) {
                        " · you are inside"
                    } else {
                        ""
                    },
                ));
            }
            for line in lines {
                push_system_chat_line(scene_state, line);
            }
            return;
        }
        SubAreaOp::Here => match subs.iter().find(|s| s.sub_area.contains(here)) {
            Some(s) => s.sub_area.id,
            None => {
                push_system_chat_line(
                    scene_state,
                    "/subarea here: no sub-area trigger holds you (you are outdoors)".into(),
                );
                return;
            }
        },
        SubAreaOp::Load(id) => id,
    };

    let Some(s) = subs.iter().find(|s| s.sub_area.id == wanted) else {
        push_system_chat_line(
            scene_state,
            format!("/subarea: zone DAT {zone_file_id} declares no sub-area {wanted:#x}"),
        );
        return;
    };
    if !s.resolves {
        push_system_chat_line(
            scene_state,
            format!(
                "/subarea {:#x}: interior DAT {} is not in this install",
                s.sub_area.id, s.sub_area.file_id
            ),
        );
        return;
    }

    // Handing the id to the latch rather than loading the DAT here keeps one
    // owner of the sub-area block, so a manual load cannot stack a second
    // interior on top of the one the player walked into. The latch governs from
    // there: an interior the player is not standing in drops again at once.
    set_sub_area.write(kuluu_render::sub_area_activation::SetSubArea {
        sub_area: Some(s.sub_area.id),
    });
    push_system_chat_line(
        scene_state,
        format!(
            "/subarea {:#x}: activating interior DAT {}…",
            s.sub_area.id, s.sub_area.file_id
        ),
    );
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
    op: crate::view_native::slash_commands::AgentControlOp,
    agent_paused: Option<&crate::view_native::AgentPaused>,
    session_event_tx: Option<&crate::view_native::SessionEventTx>,
    scene_state: &mut SceneState,
) {
    use crate::view_native::slash_commands::AgentControlOp;
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
    navmesh_state: &crate::view_native::navmesh_overlay::NavmeshState,
    self_pos: kuluu_snapshot::Vec3,
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
        let lines = kuluu_nav::zone_lines_for(z);
        if lines.is_empty() {
            push_system_chat_line(scene_state, format!("navinfo: zone {z} has no zone-lines"));
            return;
        }
        let from = kuluu_nav::glam::Vec3::new(self_pos.x, self_pos.y, self_pos.z);
        for line in lines.iter().take(6) {
            let dx = line.from_pos[0] - self_pos.x;
            let dy = line.from_pos[1] - self_pos.y;
            let dz = line.from_pos[2] - self_pos.z;
            let dist_2d = (dx * dx + dy * dy).sqrt();
            let to =
                kuluu_nav::glam::Vec3::new(line.from_pos[0], line.from_pos[1], line.from_pos[2]);
            let path_status = match kuluu_nav::NavMesh::path(&*nav, from, to) {
                Some(p) => format!("path={}wp", p.len()),
                None => "path=NONE".into(),
            };
            let name = kuluu_nav::zone_name(line.to_zone).unwrap_or("?");
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

pub(super) fn apply_keybind_update(
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

fn format_modifiers(mods: kuluu_render::Modifiers) -> &'static str {
    match (mods.ctrl, mods.alt, mods.shift, mods.super_) {
        (false, false, false, false) => "",
        (true, false, false, false) => "Ctrl+",
        (false, true, false, false) => "Alt+",
        (false, false, true, false) => "Shift+",
        (false, false, false, true) => "Super+",

        _ => "Mod+",
    }
}

fn format_zoom_status(zoom: &kuluu_render::minimap::MinimapZoom) -> String {
    match zoom.radius_yalms {
        Some(r) => format!("/minimap zoom: radius={r:.0} yalms"),
        None => "/minimap zoom: fit-to-zone".into(),
    }
}

#[cfg(feature = "enhanced-shutdown-counter")]
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

fn mirror_heal_stance(cmd: &AgentCommand, rest: &mut kuluu_render::combat_stance::RestStance) {
    use kuluu_render::combat_stance::RestKind;
    let AgentCommand::Heal { mode } = cmd else {
        return;
    };
    let next = match mode {
        kuluu_session::state::HealMode::On => RestKind::Heal,
        kuluu_session::state::HealMode::Off => match rest.kind {
            RestKind::Heal => RestKind::None,
            other => other,
        },
        kuluu_session::state::HealMode::Toggle => match rest.kind {
            RestKind::Heal => RestKind::None,
            _ => RestKind::Heal,
        },
    };
    rest.kind = next;
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
mod layout_refusal_tests {
    use super::*;
    use ffxi_proto::decode::ServerLoginMyroom as Login;
    use kuluu_render::hud::menu::{DynamicMenu, UiSound};
    use kuluu_session::myroom_layout::{
        Refusal, MESSAGE_NOT_ARRANGEABLE, MODE_GARDEN, MODE_LAYOUT, MODE_NORMAL,
    };

    fn upstairs(door_open: bool) -> SceneState {
        let mut scene = SceneState::default();
        scene.snapshot.login_state = Login::LOGIN_STATE_POLEXIT_MYROOM;
        scene.snapshot.myroom = Some(kuluu_snapshot::MyRoom {
            model: 0,
            sub_map: Login::SUB_MAP_2F,
            door_open,
        });
        scene
    }

    fn layout() -> AgentCommand {
        AgentCommand::MyroomMode { mode: MODE_LAYOUT }
    }

    #[test]
    fn layout_goes_through_upstairs_with_the_door_shut() {
        let mut scene = upstairs(false);
        let mut dynamic = DynamicMenu::default();
        assert_eq!(layout_refusal(&scene.snapshot), None);
        assert!(!layout_refused(&layout(), &mut scene, &mut dynamic));
        assert!(scene.local_toasts.is_empty() && dynamic.ui_sounds.is_empty());
    }

    #[test]
    fn an_open_door_or_layout_mode_already_on_buzzes() {
        let mut scene = upstairs(true);
        let mut dynamic = DynamicMenu::default();
        assert_eq!(layout_refusal(&scene.snapshot), Some(Refusal::DoorOpen));
        assert!(layout_refused(&layout(), &mut scene, &mut dynamic));
        assert_eq!(dynamic.ui_sounds, vec![UiSound::Buzzer]);
        assert!(scene.local_toasts.is_empty());

        let mut scene = upstairs(false);
        scene.snapshot.myroom_mode = MODE_LAYOUT;
        assert_eq!(
            layout_refusal(&scene.snapshot),
            Some(Refusal::AlreadyArranging)
        );
    }

    #[test]
    fn outside_a_house_layout_prints_the_not_arrangeable_message() {
        let mut scene = SceneState::default();
        let mut dynamic = DynamicMenu::default();
        assert_eq!(
            layout_refusal(&scene.snapshot),
            Some(Refusal::NotArrangeable)
        );
        assert!(layout_refused(&layout(), &mut scene, &mut dynamic));
        let printed: Vec<&str> = scene
            .local_toasts
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(
            printed,
            vec![dynamic
                .arrange
                .texts
                .client_message(MESSAGE_NOT_ARRANGEABLE)]
        );
        assert!(dynamic.ui_sounds.is_empty());
    }

    #[test]
    fn only_entering_layout_mode_runs_the_client_refusals() {
        let mut scene = SceneState::default();
        let mut dynamic = DynamicMenu::default();
        for mode in [MODE_NORMAL, MODE_GARDEN] {
            let command = AgentCommand::MyroomMode { mode };
            assert!(!layout_refused(&command, &mut scene, &mut dynamic));
        }
        assert!(scene.local_toasts.is_empty());
    }
}
