use super::map_screen::handle_map_key;
use super::slash_apply::{apply_keybind_update, apply_menu_text_command};
use super::*;
use kuluu_render::input_mode::ConfirmDefault;

#[derive(Debug, Clone, PartialEq)]
enum MenuDispatch {
    OpenSubmenu(MenuKind),

    /// Retail rows that "run" a command hand the runner their literal text -
    /// FFXiMain.dll retail-2026-09 RVA 0x80DC0 is the runner, and page 2's
    /// Current Time case at RVA 0x14E98C pushes its own string (.data 0x371AB8
    /// "/clock"). The same seam serves typed input later (vault §12).
    TextCommand(&'static str),

    /// myroom's Gardening/Layout rows (select ids 4 and 5 of handler FFXiMain.dll
    /// retail-2026-09 RVA 0x145460 — arms @0x145541/@0x14554A): each runs its
    /// literal through the runner (0x80DC0) and then closes myroom by name via
    /// 0x15E910, so kuluu pops the home-view level after running it (vault
    /// main_menu.md §53; the pair's literal↔id mapping was swapped in §40).
    HomeTextCommand(&'static str),

    /// A container row of the myroom panel: opens kuluu's Items window on the
    /// LSB bag named by `container` (vault main_menu.md §53 container law) —
    /// the same seam a Mog Menu storage row uses (`open_items_on_bag`).
    BrowseBag {
        container: u8,
    },

    // (`KeybindUpdate` lived here until bead kuluu-o9qt7.26.1: the Controls page
    // answers its commands at the confirm seam directly, so no dispatch routes
    // through this variant any more.)
    /// View House (page-2 row 4): the retail arm opens the Mog Menu locally —
    /// FFXiMain.dll retail-2026-09 RVA 0x14E8F5 name-opens `menu    mogcont`
    /// with no packet on the wire; kuluu's built Mog Menu is the session dialog
    /// surface fed by s2c 0x02E OPENMOGMENU (vault main_menu.md §32), so this
    /// routes through the same `AgentCommand::OpenMogMenu` seam.
    OpenMogMenu,

    /// myroom's Delivery Box row (select id 2 of handler FFXiMain.dll retail-2026-09 RVA
    /// 0x145460 → holder method .text 0x145BA0, which name-opens `menu    mogpost` at
    /// .text 0x145BAB). That window is the mail chrome riding c2s 0x04D PBX; kuluu's
    /// server-owned delivery screen answers it, opening incoming-first through the
    /// driver's Work→Check sequence (kuluu-session/src/delivery_box.rs; vault
    /// main_menu.md §53).
    OpenDeliveryBox,

    /// myroom's door row (select id 6 of handler FFXiMain.dll retail-2026-09 RVA
    /// 0x145460, arm @0x14556C): the MYROOM_IS Open/Close pair. Retail branches on
    /// the door byte at game-state +0x40C40 (predicate .text 0xF96E0); LSB never
    /// implements either Kind process() (vendor/server/src/map/packets/c2s/
    /// 0x0cb_myroom_is.cpp), so that byte has no producer and the closed-door arm —
    /// one MYROOM_IS{Kind=Open,Param1=0,Param2=Unk1} per select — is the only
    /// reachable behavior (vault main_menu.md §59 addendum 20; bead kuluu-o9qt7.13.6).
    MogHouseDoor,

    NotImplemented(String),
}

/// Header and action rows carry no value, so a cursor on one cycles nothing.
fn apply_settings_cycle(
    kind: MenuKind,
    cursor: usize,
    delta: i32,
    graphics: &mut kuluu_render::GraphicsSettings,
) {
    if let Some(field) =
        kuluu_render::hud::menu::settings_field_at(kind, cursor, graphics.dlss_supported)
    {
        graphics.cycle(field, delta);
    }
}

/// `flat_magic` is GraphicsSettings.magic_flat_list (Retail+ "Flat Magic List"); it
/// feeds the Root → submenu mapping so the drill and preview stay identical.
/// `flat_abilities` is the same pattern for GraphicsSettings.abilities_flat_list
/// and the Abilities row (vault main_menu.md §23).
fn resolve_menu_entry(
    kind: MenuKind,
    label: &str,
    flat_magic: bool,
    flat_abilities: bool,
) -> MenuDispatch {
    use kuluu_render::graphics_settings::MenuAction;
    use kuluu_render::hud::menu::{
        magic_category_for_label, COMM_EMOTE_LIST, COMM_FRIEND_LIST, MACRO_ROW_MACRO_RESULT,
        MACRO_ROW_SELECT_MACRO, ROOT_CURRENT_TIME, ROOT_LOG_OUT, ROOT_REGION_INFO, ROOT_SET_BAZAAR,
        ROOT_SHUT_DOWN, ROOT_VIEW_HOUSE, SYNTH_ROW_OPEN_LIST, SYNTH_ROW_PICK_MATERIAL,
    };
    // The Config family resolves through no labels at all: every window's rows are DAT
    // widgets read at runtime (bead kuluu-o9qt7.33; vault main_menu.md §43/§49/§57), and each
    // row carries its select semantics as a DynamicMenuAction — the hub's twelve jump-table
    // drills (§42), `conf5m`'s log-window arms 0x199264/0x19927A (§47), or an option-row store
    // id (§48). Those actions ride on confirm_menu_at_cursor's dynamic seam.
    match (kind, label) {
        (MenuKind::Config, l) if l == MenuAction::Controls.label() => {
            MenuDispatch::OpenSubmenu(MenuKind::Controls)
        }
        (MenuKind::Communication, l) if l == COMM_EMOTE_LIST => {
            MenuDispatch::OpenSubmenu(MenuKind::EmoteList)
        }
        // commenu's other list widget (DAT windowps330, caption token 61;
        // vault main_menu.md §37): drills the friend-list window, whose rows
        // arrive with the local store in `.23.1`.
        (MenuKind::Communication, l) if l == COMM_FRIEND_LIST => {
            MenuDispatch::OpenSubmenu(MenuKind::FriendList)
        }
        // Both power rows open the retail yes/no prompt first (Vault-grounded
        // reading at FFXiMain.dll retail-2026-09 RVA 0x160960 / 0x1609F0); the
        // request goes out only on Yes.
        (MenuKind::Root, ROOT_LOG_OUT) => {
            MenuDispatch::OpenSubmenu(MenuKind::PowerPrompt { shut_down: false })
        }
        (MenuKind::Root, ROOT_SHUT_DOWN) => {
            MenuDispatch::OpenSubmenu(MenuKind::PowerPrompt { shut_down: true })
        }

        // Page 2 row 10 runs the text command "/clock" through the runner
        // (FFXiMain.dll retail-2026-09 RVA 0x14E98C, literal .data 0x371AB8;
        // registry record /clock = id 0x3F in vault §12). Client-side: LSB
        // has no handler and never sees it on the wire.
        (MenuKind::Root, l) if l == ROOT_CURRENT_TIME => MenuDispatch::TextCommand("/clock"),

        // Page 2 row 3 runs the text command "/keyitem" through the same runner
        // (FFXiMain.dll retail-2026-09 RVA 0x14E855, literal .data 0x371B40;
        // registry record /keyitem = id 0x4C in vault §12). Client-side: LSB has
        // no handler and never sees it on the wire.
        (MenuKind::Root, "Key Items") => MenuDispatch::TextCommand("/keyitem"),

        // Page 2 row 2 runs the text command "/quest" through the same runner
        // (FFXiMain.dll retail-2026-09 RVA 0x14E889, literal .data 0x371B24;
        // registry record /quest = id 0x4D, whose table handler 0x7D240 resets
        // the quest-window filter word and name-opens menu quest00 — vault
        // main_menu.md §31). Client-side: LSB has no handler and never sees it.
        (MenuKind::Root, "Quests") => MenuDispatch::TextCommand("/quest"),

        // Page-2 row 4 name-opens `menu    mogcont` — the Mog Menu — after the
        // arm's veto predicate passes (FFXiMain.dll retail-2026-09 RVA 0x14E8F5;
        // vault main_menu.md §32). The gate itself runs at confirm time because
        // it reads snapshot state, like the Trade row's target check.
        (MenuKind::Root, ROOT_VIEW_HOUSE) => MenuDispatch::OpenMogMenu,

        // Page-1 row 11 name-opens `menu    region` with no gate (FFXiMain.dll
        // retail-2026-09 RVA 0x140691; vault main_menu.md §29): kuluu drills its
        // four-mode panel, opened at display mode 0.
        (MenuKind::Root, ROOT_REGION_INFO) => {
            MenuDispatch::OpenSubmenu(MenuKind::RegionInfo { mode: 0 })
        }

        // Page-2 row 1 name-opens `menu    missionm` under the game-object select
        // gate (FFXiMain.dll retail-2026-09 RVA 0x14E86D; vault main_menu.md §30).
        (MenuKind::Root, "Missions") => MenuDispatch::OpenSubmenu(MenuKind::MissionList),

        // Page-2 row 5 name-opens `menu    bazaar` with no gate (FFXiMain.dll
        // retail-2026-09 RVA 0x14E8A1; vault main_menu.md §33): kuluu drills the
        // stall picker built from entity BazaarFlag rows.
        (MenuKind::Root, ROOT_SET_BAZAAR) => MenuDispatch::OpenSubmenu(MenuKind::BazaarStalls),

        // The Map screen is a bespoke pane (no generic right-pane preview via
        // root_child_kind), so it needs its own drill arm ahead of the catch-all.
        (MenuKind::Root, "Map") => MenuDispatch::OpenSubmenu(MenuKind::Map),

        // Root categories that drill into a browsable submenu share their
        // mapping with the right-pane preview (single source of truth).
        (MenuKind::Root, label) => {
            match kuluu_render::hud::menu::root_child_kind(label, flat_magic, flat_abilities) {
                Some(submenu) => MenuDispatch::OpenSubmenu(submenu),
                None => MenuDispatch::NotImplemented(label.to_string()),
            }
        }

        // The cmbmenu launcher's rows (handler FFXiMain.dll retail-2026-09 RVA 0x1E29A0; vault
        // main_menu.md §22/§28): select id 1 opens the bag, select id 2 drills kuluu's craft list
        // in place of the cmbHwin/cmbhlst window pair.
        (MenuKind::SynthLauncher, l) if l == SYNTH_ROW_PICK_MATERIAL => {
            MenuDispatch::OpenSubmenu(MenuKind::Items)
        }
        (MenuKind::SynthLauncher, l) if l == SYNTH_ROW_OPEN_LIST => {
            MenuDispatch::OpenSubmenu(MenuKind::CraftList)
        }

        // The mcrmenu launcher rows drill the §41 windows built by bead .19.1
        // (handler FFXiMain.dll retail-2026-09 RVA 0x1A64B0 items {1,2}; vault
        // main_menu.md §34/§41): Select Macro opens the store-backed mcrselec,
        // Macro Result the grounded-empty mcres20 surface.
        (MenuKind::MacrosPalette, l) if l == MACRO_ROW_SELECT_MACRO => {
            MenuDispatch::OpenSubmenu(MenuKind::MacroSelect)
        }
        (MenuKind::MacrosPalette, l) if l == MACRO_ROW_MACRO_RESULT => {
            MenuDispatch::OpenSubmenu(MenuKind::MacroResult)
        }

        // myroom's select family (handler FFXiMain.dll retail-2026-09 RVA
        // 0x145460, ids 1..7): the six container rows open their LSB bags, the
        // two text-command rows run their literals and close the panel, Change
        // Jobs name-opens `jobchang` (id3 arm @0x145517 — its predicate only
        // selects an extra helper call 0xA1860(50); BOTH paths open the window,
        // so kuluu opens it ungated). Delivery Box (id 2) name-opens `mogpost` — the
        // mail chrome over c2s 0x04D PBX (§53; bead kuluu-o9qt7.13.4). The door row
        // (id 6) sends its MYROOM_IS select, choosing Kind from the door byte (beads
        // kuluu-o9qt7.13.6/.13.6.1); the Remodel row
        // (id 7, arm @.text 0x1454E7) name-opens `roomlist` (bead kuluu-o9qt7.13.5).
        // Every select id of the handler now answers.
        (MenuKind::HomeView, _) => {
            if let Some(container) = kuluu_render::hud::menu::home_view_row_container(label) {
                MenuDispatch::BrowseBag { container }
            } else if let Some(cmd) = kuluu_render::hud::menu::home_view_text_command(label) {
                MenuDispatch::HomeTextCommand(cmd)
            } else if label == "Change Jobs" {
                MenuDispatch::OpenSubmenu(MenuKind::JobChange)
            } else if label == kuluu_render::hud::menu::HOME_ROW_DELIVERY_BOX {
                MenuDispatch::OpenDeliveryBox
            } else if label == kuluu_render::hud::menu::HOME_ROW_OPEN_MOG_HOUSE {
                MenuDispatch::MogHouseDoor
            } else if label == kuluu_render::hud::menu::HOME_ROW_REMODEL {
                MenuDispatch::OpenSubmenu(MenuKind::RoomList)
            } else {
                MenuDispatch::NotImplemented(label.to_string())
            }
        }

        // `jobchang`'s two rows (DAT 'jobc' widgets windowps273/274, catalog
        // tokens 65/66) drill their pick lists — Main Job and Support Job each
        // send one c2s 0x100 MYROOM_JOB on select (vault main_menu.md §53).
        (MenuKind::JobChange, l) if l == kuluu_render::hud::menu::JOB_CHANGE_ROWS[0] => {
            MenuDispatch::OpenSubmenu(MenuKind::JobPick { main_pick: true })
        }
        (MenuKind::JobChange, l) if l == kuluu_render::hud::menu::JOB_CHANGE_ROWS[1] => {
            MenuDispatch::OpenSubmenu(MenuKind::JobPick { main_pick: false })
        }

        // mgcmenu select: the category's school id (jump table 0x13F7CC; vault
        // main_menu.md §21) becomes the filter of the `magic` spell list.
        (MenuKind::MagicGroups, label) => match magic_category_for_label(label) {
            Some(school) => MenuDispatch::OpenSubmenu(MenuKind::MagicGroup(school)),
            None => MenuDispatch::NotImplemented(label.to_string()),
        },

        // abimenu select: the category row becomes the `ability` window's group
        // filter (FFXiMain.dll retail-2026-09 RVA 0x13FA70; vault main_menu.md §23).
        (MenuKind::AbilitiesGroups, label) => {
            match kuluu_render::hud::menu::abilities_category_index(label) {
                Some(idx) => MenuDispatch::OpenSubmenu(MenuKind::AbilitiesGroup(idx as u8)),
                None => MenuDispatch::NotImplemented(label.to_string()),
            }
        }

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

        // The Controls page no longer resolves by label (bead kuluu-o9qt7.26.1):
        // it is dynamic now — its body lists the live key map (read-only rows) and
        // its four command rows carry `ControlsPreset`/`ControlsReset` actions, all
        // consumed at the confirm seam beside `A::DisplayInfo`.
        (_, other) => MenuDispatch::NotImplemented(other.to_string()),
    }
}

const EQUIP_SLOT_INDEX_MAX: u8 = (kuluu_render::equip_slot::EquipmentIndex::ALL.len() - 1) as u8;

/// Apply one statcom2 select (vault main_menu.md §59 FFXiMain.dll retail-2026-09
/// handler RVA 0x148D00 over detail table RVA 0x3716B8): Play Time runs the
/// `/playtime` chat command and opens nothing; re-selecting the row whose
/// detail already shows changes nothing; any other select closes the previous
/// display-only detail (helper RVA 0x148AC0) by REPLACING it, then opens the
/// new one beside the selector. The Profile window's former overlay lives on
/// as this detail level's own rows; retail keeps persona + level + detail on
/// screen together, and kuluu derives those panels' visibility from the stack
/// (status_flow_open), so no separate overlay flag exists anymore.
/// The currency-request seam (bead kuluu-o9qt7.35): a Currencies-family select fires its
/// header-only report request on **every** select — the retail opener runs first and caches
/// nothing (FFXiMain.dll retail-2026-09 `.text 0xFD520`/`.text 0xFD550`; one-shot replies s2c
/// 0x113/0x118), even when the window itself dedupes open. Other rows request nothing.
fn currency_request_mode_for(
    entry_kind: kuluu_render::hud::status_panel::StatusEntryKind,
) -> Option<u8> {
    use kuluu_render::hud::currency_law::CurrencyMode;
    use kuluu_render::hud::status_panel::StatusEntryKind as K;
    match entry_kind {
        K::Currencies => Some(CurrencyMode::One.as_wire()),
        K::Currencies2 => Some(CurrencyMode::Two.as_wire()),
        _ => None,
    }
}

fn open_statcom_detail(stack: &mut MenuStack, cursor: usize) {
    use kuluu_render::hud::status_panel::{StatusEntryKind, STATUS_ENTRIES};
    let Some(entry_idx) = STATUS_ENTRIES.get(cursor).map(|_| cursor as u8) else {
        return;
    };
    if matches!(
        STATUS_ENTRIES[usize::from(entry_idx)].kind,
        StatusEntryKind::PlayTime
    ) {
        // Handled by the caller's chat side effect; never opens a window (§2 id 9).
        return;
    }
    let detail = MenuKind::StatusDetail { entry: entry_idx };
    if let Some(top) = stack.levels.last() {
        if let MenuKind::StatusDetail { entry: prev } = top.kind {
            if usize::from(prev) == usize::from(entry_idx) {
                return; // re-selecting the same row is a no-op (§2 step 3)
            }
            stack.pop();
        }
    }
    stack.push(detail);
}

/// Status rows 11 and 13 open the Unity and alter ego point windows
/// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md), rows 7 and 8
/// the merit and job point windows
/// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md); every
/// other row opens its detail window.
fn open_status_row(
    stack: &mut MenuStack,
    cursor: usize,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    dynamic: &mut kuluu_render::hud::menu::DynamicMenu,
) {
    use kuluu_render::hud::merit_windows;
    use kuluu_render::hud::status_panel::{StatusEntryKind, STATUS_ENTRIES};
    use kuluu_render::hud::unity_fp;
    let outcome = match STATUS_ENTRIES.get(cursor).map(|e| e.kind) {
        Some(StatusEntryKind::Unity) => unity_fp::open_unity_menu(stack, &mut scene_state.snapshot),
        Some(StatusEntryKind::FacetCategories) => {
            unity_fp::open_alter_ego_categories(stack, &mut scene_state.snapshot)
        }
        Some(StatusEntryKind::MeritPoints) => {
            let outcome = merit_windows::open_merit_menu(stack, dynamic, &scene_state.snapshot);
            apply_merit_outcome(outcome, scene_state, cmd_tx);
            return;
        }
        Some(StatusEntryKind::JobPoints) => {
            let outcome = merit_windows::open_job_point_jobs(stack, &scene_state.snapshot);
            apply_merit_outcome(outcome, scene_state, cmd_tx);
            return;
        }
        _ => {
            open_statcom_detail(stack, cursor);
            return;
        }
    };
    apply_unity_fp_outcome(outcome, scene_state, cmd_tx);
}

/// Sends what a merit or job point window asked for and prints its client messages.
fn apply_merit_outcome(
    outcome: kuluu_render::hud::merit_windows::MeritOutcome,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) {
    for request in outcome.requests {
        if let Err(e) = cmd_tx.try_send(merit_command(request)) {
            push_system_chat_line(scene_state, format!("[menu] request dropped: {e}"));
        }
    }
    for line in outcome.chat_lines {
        push_system_chat_line(scene_state, line);
    }
}

fn merit_command(request: kuluu_render::hud::merit_windows::MeritRequest) -> AgentCommand {
    use kuluu_render::hud::merit_windows::MeritRequest as R;
    match request {
        R::SetMode { limit_points } => AgentCommand::SetMeritMode { limit_points },
        R::Edit { index, raise } => AgentCommand::EditMerit { index, raise },
        R::SpendJobPoint { type_id } => AgentCommand::SpendJobPoint { type_id },
        R::RequestJobPoints => AgentCommand::RequestJobPoints,
    }
}

/// Sends what a Unity / alter ego point window asked for and prints its client messages.
fn apply_unity_fp_outcome(
    outcome: kuluu_render::hud::unity_fp::UnityFpOutcome,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) {
    for request in outcome.requests {
        if let Err(e) = cmd_tx.try_send(unity_fp_command(request)) {
            push_system_chat_line(scene_state, format!("[menu] request dropped: {e}"));
        }
    }
    for line in outcome.chat_lines {
        push_system_chat_line(scene_state, line);
    }
}

fn unity_fp_command(request: kuluu_render::hud::unity_fp::UnityFpRequest) -> AgentCommand {
    use kuluu_render::hud::unity_fp::UnityFpRequest as R;
    match request {
        R::UnityRanking(result_set) => AgentCommand::RequestUnityRanking { result_set },
        R::UnityQuest => AgentCommand::RequestUnityQuest,
        R::UnityChat(mode) => AgentCommand::SetUnityChat { mode },
        R::AlterEgoPoints => AgentCommand::RequestAlterEgoPoints,
        R::UpgradeAlterEgo(upgrade_id) => AgentCommand::UpgradeAlterEgo { upgrade_id },
    }
}

/// Sends the Unity chat panel's throttled mode changes and, once per zone-in, the stored mode
/// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md). The client option
/// store lives for the session, so until the panel stores a mode there is none to send.
pub fn unity_chat_send_system(
    time: Res<Time>,
    cmd_tx: Res<crate::view_native::input::CommandTx>,
    scene_state: Res<SceneState>,
    mut dynamic: ResMut<kuluu_render::hud::menu::DynamicMenu>,
    mut last_zone_generation: Local<Option<u64>>,
) {
    use kuluu_render::hud::unity_fp::UNITY_CHAT_OPTION;
    if dynamic.unity_fp.chat.is_some() {
        let seconds = time.delta_secs();
        if let Some(mode) = dynamic
            .unity_fp
            .chat
            .as_mut()
            .and_then(|chat| chat.advance(seconds))
        {
            let _ = cmd_tx.0.try_send(AgentCommand::SetUnityChat { mode });
        }
    }
    let snap = &scene_state.snapshot;
    if snap.zone_id.is_some() && *last_zone_generation != Some(snap.zone_generation) {
        *last_zone_generation = Some(snap.zone_generation);
        if let Some(&stored) = dynamic.config_option_values.get(&UNITY_CHAT_OPTION) {
            let _ = cmd_tx.0.try_send(AgentCommand::SetUnityChat {
                mode: u8::from(stored != 0),
            });
        }
    }
}

/// Sends what the arrange editor asked for, in order: c2s 0x0FA for each piece placed, c2s 0x0FB for
/// each piece put away, and the return to normal play
/// (.agents/skills/retail-observe/references/2026-10-15-mog-house-arrange.md B8).
pub fn arrange_request_send_system(
    cmd_tx: Res<crate::view_native::input::CommandTx>,
    mut scene_state: ResMut<SceneState>,
    mut dynamic: ResMut<kuluu_render::hud::menu::DynamicMenu>,
) {
    if dynamic.arrange.requests.is_empty() {
        return;
    }
    for request in std::mem::take(&mut dynamic.arrange.requests) {
        if let Err(e) = cmd_tx.0.try_send(arrange_command(request)) {
            push_system_chat_line(&mut scene_state, format!("[layout] request dropped: {e}"));
        }
    }
}

fn arrange_command(request: kuluu_render::myroom_arrange::ArrangeRequest) -> AgentCommand {
    use kuluu_render::myroom_arrange::ArrangeRequest as R;
    match request {
        R::Layout {
            item_no,
            slot,
            container,
            floor,
            col,
            level,
            row,
            rot,
        } => AgentCommand::MyroomLayout {
            item_no,
            slot,
            container,
            floor,
            col,
            level,
            row,
            rotation: rot,
        },
        R::Bankin {
            item_no,
            slot,
            container,
        } => AgentCommand::MyroomBankin {
            item_no,
            slot,
            container,
        },
        R::Leave => AgentCommand::MyroomMode {
            mode: kuluu_session::myroom_layout::MODE_NORMAL,
        },
    }
}

/// While a DISPLAY-ONLY statcom2 companion tops the stack, it must not hold
/// the keys — they stay on the selector underneath (focus law, vault
/// main_menu.md §59: display-only windows are skipped; only these six are,
/// per the FFXiMain.dll retail-2026-09 detail table behind RVA 0x148D00).
/// Returns the index of the Status level to drive instead.
fn statcom_selector_below(stack: &MenuStack) -> Option<usize> {
    let n = stack.levels.len();
    if n < 2 {
        return None;
    }
    let top = stack.levels[n - 1].kind;
    let MenuKind::StatusDetail { entry } = top else {
        return None;
    };
    let entry_def = kuluu_render::hud::status_panel::STATUS_ENTRIES.get(usize::from(entry))?;
    if !kuluu_render::hud::status_panel::status_detail_display_only(entry_def.kind) {
        return None;
    }
    (stack.levels[n - 2].kind == MenuKind::Status).then_some(n - 2)
}

/// Confirm the entry under the cursor. Retail lists Use for every item and
/// refuses an unusable or cooling-down one silently: no sub-target cursor, no
/// chat line, the submenu stays put. The item drop confirm unwinds both the
/// confirm and the Item submenu back to the list on either answer.
pub(super) fn confirm_menu_at_cursor(
    bindings: &mut Bindings,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    keybinds_state: &mut KeybindsStateRes,
    graphics: &mut kuluu_render::GraphicsSettings,
    hud_panels: &mut kuluu_render::hud::HudPanels,
    net_status: &mut kuluu_render::hud::network_status::NetStatusVisible,
    audio_mute: &mut kuluu_render::audio::AudioMuteState,
    vana_clock: &mut kuluu_render::vana_time::VanaClock,
    vana_clock_visible: &mut kuluu_render::hud::vana_clock::VanaClockVisible,
    dynamic: &mut kuluu_render::hud::menu::DynamicMenu,
    target_id: Option<u32>,
    self_pos: kuluu_snapshot::Vec3,
    command_surface: &crate::view_native::command_surface::CommandSurface,
    item_bag: &mut kuluu_render::hud::item_screen::ItemScreenContainer,
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
        if handle_retail_plus_row(label, graphics, scene_state) {
            return None;
        }
        toggle_debug_panel(
            label,
            hud_panels,
            net_status,
            audio_mute,
            vana_clock,
            self_pos,
            scene_state,
        );
        return None;
    }

    if matches!(kind, MenuKind::Status) {
        // The live Status window is `statcom2`: its select handler (FFXiMain.dll
        // retail-2026-09 RVA 0x148D00) drives the detail table RVA 0x3716B8, so
        // rows answer by default. The persona WINDOW alone answers no command
        // events (RVA 0x12AC60 is a ret-8 stub); the old kuluu panel-wide
        // silence lives on only as the opt-out flip of "Status Rows".
        if !graphics.interactive_status_rows {
            return None;
        }
        use kuluu_render::hud::status_panel::{StatusEntryKind, STATUS_ENTRIES};
        match STATUS_ENTRIES.get(cursor).map(|e| e.kind) {
            // §2 id 9: Play Time runs `/playtime` and opens no window.
            Some(StatusEntryKind::PlayTime) => {
                let line =
                    kuluu_render::hud::status_panel::play_time_chat_line(&scene_state.snapshot);
                push_system_chat_line(scene_state, line);
            }
            // A gated row answers nothing at all: retail disables it (grey + the
            // select never reaches the detail table), so kuluu neither opens a window
            // nor runs a command for it (`statcom2` open code FFXiMain.dll retail-2026-09
            // `.text 0x148B50`; re-read this cycle).
            Some(entry_kind)
                if kuluu_render::hud::status_panel::status_row_enabled(
                    entry_kind,
                    &scene_state.snapshot,
                ) =>
            {
                if let Some(mode) = currency_request_mode_for(entry_kind) {
                    // `try_send`, never async `send`: this seam is synchronous — a dropped
                    // (never-polled) Send future enqueues nothing.
                    let _ = cmd_tx.try_send(AgentCommand::RequestCurrencies { mode });
                }
                open_status_row(stack, cursor, scene_state, cmd_tx, dynamic);
            }
            // Guard failed: the row is grey, so the select is inert.
            _ => {}
        }
        return None;
    }
    if kuluu_render::hud::unity_fp::is_unity_fp_window(kind) {
        let action = kuluu_render::hud::menu::entry_action(kind, cursor, dynamic);
        let outcome = kuluu_render::hud::unity_fp::select_unity_fp_row(
            kind,
            action,
            stack,
            dynamic,
            &mut scene_state.snapshot,
        );
        apply_unity_fp_outcome(outcome, scene_state, cmd_tx);
        return None;
    }
    if let MenuKind::Merit(window) = kind {
        let action = kuluu_render::hud::menu::entry_action(kind, cursor, dynamic);
        let outcome = kuluu_render::hud::merit_windows::select_merit_row(
            window,
            action,
            stack,
            dynamic,
            &scene_state.snapshot,
        );
        apply_merit_outcome(outcome, scene_state, cmd_tx);
        return None;
    }
    if let MenuKind::Arrange(_) = kind {
        return kuluu_render::myroom_arrange::select_arrange_row(stack, dynamic);
    }
    if matches!(
        kind,
        MenuKind::Config | MenuKind::Graphics | MenuKind::GraphicsDlss
    ) {
        use kuluu_render::graphics_settings::{MenuAction, MenuRow};
        match kuluu_render::hud::menu::settings_row_at(kind, cursor, graphics.dlss_supported) {
            Some(MenuRow::Action(MenuAction::ResetToMinimum)) => {
                graphics.reset_to_minimum();
                push_system_chat_line(scene_state, "[menu] Graphics reset to Minimum".into());
            }
            Some(MenuRow::Action(MenuAction::ResetDlssConfig)) => {
                graphics.reset_dlss_config();
                push_system_chat_line(scene_state, "[menu] DLSS config reset to defaults".into());
            }
            Some(MenuRow::Action(MenuAction::DlssConfig)) => stack.push(MenuKind::GraphicsDlss),
            Some(MenuRow::Action(MenuAction::Controls)) => stack.push(MenuKind::Controls),
            _ => apply_settings_cycle(kind, cursor, 1, graphics),
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
            // Status detail windows are pure readouts: their rows hold no
            // command (FFXiMain.dll retail-2026-09 RVA 0x148D00 opens only),
            // so a press keeps the window open and changes nothing.
            if matches!(action, A::DisplayInfo) {
                return None;
            }
            // kuluu's Controls-page commands (bead kuluu-o9qt7.26.1): applied
            // through the same seam as `/keybinds preset|reset`, then KEEP the page
            // open — `refresh_dynamic_menu_rows` re-lists its live rows on top of
            // the new store, so a remap shows immediately (the bead's "after
            // change, without restart" law). Retail has no such rows: it assigns
            // one key per row (.text 0x19CFA0 writing across the link group; vault
            // main_menu.md §59 addendum 25) — page-3 kuluu-only surface.
            if matches!(action, A::ControlsPreset(_) | A::ControlsReset) {
                let update = match action {
                    A::ControlsPreset(preset) => KeybindUpdate::Preset(preset),
                    _ => KeybindUpdate::Reset,
                };
                apply_keybind_update(update, bindings, keybinds_state, scene_state);
                return None;
            }
            // The `abisortw` chain row (bead kuluu-o9qt7.34): retail's handler answers
            // only this one select, writing the paired list's state word `[emote+0x6C]=1`
            // unconditionally — even mid-arrangement it just re-writes 1, dropping any
            // half-picked pair (FFXiMain.dll retail-2026-09 `.text 0x223CF0` id-5 arm →
            // `.text 0x223DC3`). Both windows stay up.
            if matches!(action, A::AbisortwSelect { .. }) {
                dynamic.emote_sort_state = kuluu_render::hud::menu::EmoteSortState::AwaitFirst;
                return None;
            }
            // While the swap state is pending, an emote-row press belongs to the FSM —
            // retail swaps the whole handler family at `[emote+0x6C]` (`.text 0x225EC0`
            // awaits the first pick, `.text 0x225E00` the second), and firing a canned
            // emote is not an arm of either. Off restores today's fire path unchanged.
            if let A::Emote { emote_id } = action {
                use kuluu_render::hud::menu::EmoteSortState as ES;
                match dynamic.emote_sort_state {
                    ES::Off => {}
                    ES::AwaitFirst => {
                        // The pick lands where retail keeps `[emote+0x70]` — the pending
                        // id — and the list waits for its partner.
                        dynamic.emote_sort_state = ES::AwaitSecond(emote_id);
                        return None;
                    }
                    ES::AwaitSecond(first) => {
                        // The guard law (`.text 0x2258D0`): equal row ids or a missing id
                        // no-op without touching rows, state or store — retail stays put.
                        if let Some(order) = kuluu_render::hud::menu::committed_emote_swap(
                            &scene_state.snapshot,
                            first,
                            emote_id,
                        ) {
                            // Persist the blob (client-local; CommitEmoteOrder rewrites
                            // `emote_order.json` and folds it back), then retail's own
                            // tail: refill (.text 0x2256F0) with the state reset by fill
                            // law (.text 0x225960) and select row B — the second pick.
                            if let Err(e) = cmd_tx
                                .try_send(AgentCommand::CommitEmoteOrder { ids: order.clone() })
                            {
                                push_system_chat_line(
                                    scene_state,
                                    format!("[menu] emote-order commit dropped: {e}"),
                                );
                            }
                            scene_state.snapshot.emote_order = order;
                            let mut rows =
                                kuluu_render::hud::menu::emote_rows(&scene_state.snapshot);
                            kuluu_render::hud::menu::append_abisortw_chain_row(
                                &mut rows,
                                dynamic.abisortw_captions.get(1).and_then(|c| c.as_deref()),
                            );
                            if let Some(pos) = rows.iter().position(|row| {
                                matches!(row.action, A::Emote { emote_id: id } if id == emote_id)
                            }) {
                                if let Some(level) = stack.current_mut() {
                                    level.cursor = pos;
                                }
                            }
                            dynamic.rows = rows;
                            dynamic.emote_sort_state = ES::Off;
                        }
                        return None;
                    }
                }
            }
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
            // The trade window's own-slot rows (FFXiMain.dll retail-2026-09
            // handler 0x1515E0 gates every select on state==3; vault
            // main_menu.md §25): a filled slot releases via the qty-0 TRADE_LIST
            // path, an empty one opens that slot's Add-Item picker.
            if let A::TradeOwnSlot { trade_index } = action {
                let started = scene_state
                    .snapshot
                    .trade
                    .as_deref()
                    .is_some_and(|t| !t.incoming_pending && t.active);
                if !started {
                    return None;
                }
                let filled = scene_state
                    .snapshot
                    .trade
                    .as_deref()
                    .and_then(|t| t.my_slots.get(trade_index as usize).copied().flatten())
                    .is_some();
                if filled {
                    if let Err(e) = cmd_tx.try_send(AgentCommand::TradeClearSlot { trade_index }) {
                        push_system_chat_line(
                            scene_state,
                            format!("[menu] trade release dropped: {e}"),
                        );
                    }
                } else {
                    stack.push(MenuKind::TradePick { trade_index });
                }
                return None;
            }
            // An Add-Item pick stages ONE unit into the slot — retail's qty
            // keypad is [I-pending] (vault main_menu.md §25) and 1 is the
            // minimum legal ItemNum under player_trade.cpp setSlot. Like
            // retail's re-pushing views, the picker stays open for more picks.
            if let A::TradeStageFromBag {
                trade_index,
                item_no,
                qty,
                bag_slot,
            } = action
            {
                if let Err(e) = cmd_tx.try_send(AgentCommand::TradeStage {
                    trade_index,
                    item_no,
                    qty,
                    bag_slot,
                }) {
                    push_system_chat_line(scene_state, format!("[menu] trade stage dropped: {e}"));
                }
                return None;
            }
            // The region-info window answers a select by installing the display table on
            // itself — FFXiMain.dll retail-2026-09 RVA 0x140780 tail-jumps 0x1E7980 rather than
            // opening anything (vault main_menu.md §29) — so kuluu swaps the payload in place.
            if let A::RegionInfoMode { mode } = action {
                if let Some(level) = stack.current_mut() {
                    level.kind = MenuKind::RegionInfo { mode };
                }
                return None;
            }

            // The missionm select tail name-opens miss00 over the list (FFXiMain.dll
            // retail-2026-09 RVA 0x22DB40; vault main_menu.md §30) — kuluu pushes the
            // per-port detail window the same way.
            if let A::MissionOpenLog { port } = action {
                stack.push(MenuKind::MissionDetail { port });
                return None;
            }

            // The quest00 select tail name-opens quest01 over the list with the
            // row value (FFXiMain.dll retail-2026-09 RVA 0x2300EE; vault
            // main_menu.md §31) — kuluu pushes that quest's detail the same way.
            if let A::QuestOpenDetail { log, id } = action {
                stack.push(MenuKind::QuestDetail { log, id });
                return None;
            }

            // Config-family drill rows push their own window (handler jump tables:
            // hub FFXiMain.dll retail-2026-09 RVA 0x196770/.text 0x1968A4, `conf5m`
            // 0x199240; vault main_menu.md §42/§47) — the page's DAT rows arrive on
            // the next refresh of refresh_dynamic_menu_rows.
            if let A::OpenRetailConfigPage { page } = action {
                stack.push(MenuKind::RetailConfigPage(page));
                return None;
            }
            if let A::OpenRetailConfigSub { sub } = action {
                stack.push(MenuKind::RetailConfigSub(sub));
                return None;
            }
            // A companion drill writes retail's mode dword and pushes the window
            // the handler leaves focused — its manager-open order ends with that
            // window (FFXiMain.dll retail-2026-09 RVA 0x15E350 calls at .text
            // 0x1985A8/0x1985BB of handler RVA 0x1984F0; vault main_menu.md §59
            // addendum 5). Retail's `textcol1` cancel arm closes its sibling by
            // name (.text 0x19888E/0x19889D), so the pair leaves as one level.
            if let A::OpenRetailConfigCompanion { mode, opens } = action {
                if let Some(&focused) = opens.last() {
                    dynamic.config_companion_modes.insert(focused, mode);
                    stack.push(MenuKind::RetailConfigSub(focused));
                }
                return None;
            }
            // The dialog row raises its Yes/No prompt on the spot and writes nothing
            // yet — retail's arm .text 0x19B309 only calls the dialog service RVA
            // 0x210E30 and returns; the callback is where the answer lands (vault
            // main_menu.md §59 addenda 7–8). The prompt opens on No, like every other
            // retail confirm.
            if matches!(action, A::OpenKeyAssignResetPrompt) {
                stack.open_confirm(MenuKind::KeyAssignResetPrompt, ConfirmDefault::No);
                return None;
            }
            // A roomlist style row raises its Yes/No prompt on the spot and sends
            // nothing yet — retail's row callback .text FFXiMain.dll retail-2026-09
            // 0x1D6B20 calls the dialog service .text 0x210E30 (prompt "Make
            // changes?"), and only the yes-callback wrapper .text 0x1D6AB0 reaches
            // the MYROOM_IS sender (vault main_menu.md §59 addendum 20/21; bead
            // kuluu-o9qt7.13.5). The prompt opens on No, like every retail confirm.
            if let A::MogHouseRemodelSelect { style } = action {
                // The patio row keeps its bell: retail leaves that row dim and turns its
                // select into nothing but the error bell (.text FFXiMain.dll retail-2026-09
                // 0x1D6B63), so with no key item 3051 there is no prompt and no send
                // (bead kuluu-o9qt7.13.5.1).
                let gated = stack.current().is_some_and(|level| {
                    kuluu_render::hud::menu::menu_row_disabled(
                        MenuKind::RoomList,
                        level.cursor,
                        &scene_state.snapshot,
                    )
                });
                if gated {
                    return None;
                }
                stack.open_confirm(MenuKind::RoomListConfirm { style }, ConfirmDefault::No);
                return None;
            }
            // A radio row of a law'd config page writes its record through the
            // store wrapper FFXiMain.dll retail-2026-09 RVA 0x193830(store_id,
            // value) (vault main_menu.md §58; bead kuluu-o9qt7.20.1): the write
            // is silent, keeps the window open, and stays on this machine —
            // LSB's c2s 0x0DC grammar accepts only the named SAVE_CONF bits
            // (vendor/server/src/map/packets/c2s/0x0dc_config.cpp), never store
            // options. The row marker arrives with the next refresh. The one
            // exception is `conf12wi` rows 3/4, whose arm rides out to helper
            // 0x1A8030 after the write — handled below.
            if let A::ConfigOptionSet { store_id, value } = action {
                // The BF pair's announce mode reads the value the store held
                // *before* the primary write (.text FFXiMain.dll retail-2026-09
                // RVA 0x19BA1D grabs it through wrapper 0x193850 first), so take
                // it before inserting. A never-read store is `None`, which the law
                // treats as not-1 exactly like retail's fresh config.dat dword.
                let bf_pair = (store_id == kuluu_render::hud::menu::CONFIG_OPTION_CONF_BF_PAIR)
                    .then(|| dynamic.config_option_values.get(&store_id).copied());
                dynamic.config_option_values.insert(store_id, value);
                if let Some(old) = bf_pair {
                    // Helper FFXiMain.dll retail-2026-09 RVA 0x1A8030: the arm hands
                    // it {mode} only — and a Monstrosity session bails before
                    // anything reaches the wire (.text 0x1A8031 calls predicate
                    // 0x97D50, .text 0x1A8036 returns false), leaving the store write
                    // in place. The >0x2328 ms guard (.text 0x1A7D05) then decides:
                    // on suppression retail pokes sub-window id 0x22 through drawer
                    // singleton [0x10578158] (.text 0x1A8058–0x1A8068) — chrome with
                    // no kuluu counterpart, so the announce is dropped; on pass retail
                    // stamps its clock global ([0x106627e4] store at .text 0x1A806E)
                    // and sends through writer 0xE5520. The helper's tail re-persist
                    // of the same store value (.text 0x1A8094/0x1A80B8) is the write
                    // already applied above (vault main_menu.md §59 addendum 16).
                    if scene_state.snapshot.monstrosity.is_none() {
                        let elapsed = dynamic
                            .conf_bf_stamp
                            .map(|stamp| stamp.elapsed().as_millis() as u64);
                        if kuluu_render::hud::menu::conf_bf_announce_allowed(elapsed) {
                            dynamic.conf_bf_stamp = Some(std::time::Instant::now());
                            let mode = kuluu_render::hud::menu::conf_bf_packet_mode(value, old);
                            if let Err(e) =
                                cmd_tx.try_send(AgentCommand::AnnounceConfigBfPair { mode })
                            {
                                push_system_chat_line(
                                    scene_state,
                                    format!("[menu] bf pair announce dropped: {e}"),
                                );
                            }
                        }
                    }
                }
                return None;
            }
            // The bit pairs (`conf12wi` rows 7/8 and 13/14, `conf3win` rows 12/13)
            // answer through the read-modify-write helper FFXiMain.dll retail-2026-09
            // RVA 0x1938F0: its callee .text 0x193910 reads option
            // CONFIG_OPTION_MISC_FLAGS through wrapper 0x193850, sets or clears one
            // bit, and writes the dword back through 0x193830 (vault main_menu.md §59
            // addendum 5). One shared mask, several pairs; still client-local — LSB's
            // c2s 0x0DC accepts only the named SAVE_CONF bits (bead kuluu-o9qt7.20.1).
            if let A::ConfigOptionBitSet { bit, value } = action {
                let mask = dynamic
                    .config_option_values
                    .get(&kuluu_render::hud::menu::CONFIG_OPTION_MISC_FLAGS)
                    .copied()
                    .unwrap_or(0) as u32;
                let updated = if value == 0 {
                    mask & !(1u32 << bit)
                } else {
                    mask | (1u32 << bit)
                };
                dynamic.config_option_values.insert(
                    kuluu_render::hud::menu::CONFIG_OPTION_MISC_FLAGS,
                    updated as i32,
                );
                return None;
            }
            // A filter-list page row (`fxfilter`) flips one bit of the single option dword its
            // window persists: class C's store method (.text FFXiMain.dll retail-2026-09 RVA
            // 0x198390) writes `[obj+0x30]` back through wrapper 0x193830 and its mark rides the
            // bit test .text 0x193960 (vault main_menu.md §59 addendum 14; bead kuluu-o9qt7.20.1.3).
            // Silent, window stays open, nothing on the wire — LSB has no packet for this store
            // (`vendor/server/src/map/packets/c2s/0x0db_config_language.cpp` carries only
            // MessageFilter/MessageFilter2).
            if let A::ConfigOptionToggleBit { store_id, bit } = action {
                let mask = dynamic
                    .config_option_values
                    .get(&store_id)
                    .copied()
                    .unwrap_or(0) as u32;
                dynamic
                    .config_option_values
                    .insert(store_id, (mask ^ (1u32 << bit)) as i32);
                return None;
            }
            // A `cfilter` row select runs class B's shared select method (.text FFXiMain.dll
            // retail-2026-09 RVA 0x198080, vtable slot +0x4c) verbatim through the exported law
            // transition — group store rule, console-bit toggle with the param-2 master flip,
            // plain pair-lane toggle, interlock three-state cycle (vault main_menu.md §59 addendum
            // 15; bead kuluu-o9qt7.20.1.3). Silent like every other config write: LSB has no
            // packet for the five cfilter stores (`vendor/server/src/map/packets/c2s/0x0db_config_language.cpp`
            // carries only MessageFilter/MessageFilter2), and the shared select method itself
            // never closes the window — it writes the in-object dwords and returns (.text
            // FFXiMain.dll retail-2026-09 RVA 0x198080–0x19822F rets on every class).
            if let A::ConfigCfilterSelect { table_idx } = action {
                kuluu_render::hud::menu::cfilter_apply_select(
                    table_idx as usize,
                    &mut dynamic.config_option_values,
                );
                return None;
            }
            // The window-local pair rows (`conf3win` ids 3/4, arms .text FFXiMain.dll
            // retail-2026-09 RVA 0x19aa85/0x19aa92) write only their own object field
            // — kuluu keeps the choice in `config_window_choices`: never the option
            // store, never the wire; the window stays open like retail's (vault
            // main_menu.md §59 addendum 9).
            if let A::ConfigWindowChoiceSet {
                window_key,
                obj_off,
                value,
            } = action
            {
                dynamic
                    .config_window_choices
                    .insert((window_key, obj_off), value);
                return None;
            }
            // A slider row's select arm does NOT write — it pins the window into
            // edit mode on that store (conf2win entry arms 3–7 only set the edit
            // bytes/mode and stash the widget; handler FFXiMain.dll retail-2026-09
            // RVA 0x197470). The pin law lives in `handle_menu_key`; edits from
            // vault main_menu.md §58 fill in an unknown starting value.
            if let A::ConfigSliderEdit { store_id } = action {
                if dynamic.option_edit.is_none() {
                    // Slider stores only ever hold step values 0..=100; the
                    // negative off-value lawed on `conf4` lives on its
                    // level-only store 0x41 and never reaches a pin (bead
                    // kuluu-o9qt7.20.1).
                    let value = dynamic
                        .config_option_values
                        .get(&store_id)
                        .copied()
                        .map(|v| v as u8)
                        .unwrap_or_else(|| {
                            kuluu_render::hud::menu::config_slider_boot_default(store_id)
                        });
                    dynamic.option_edit = Some((store_id, value));
                }
                return None;
            }
            // Leaf option rows: retail changes the value in place with no window
            // event (vault main_menu.md §46/§48), and until the option store lands
            // on bead kuluu-o9qt7.20.1 kuluu answers them with the standard line,
            // keeping its window open like retail does.
            if let A::ConfigOptionRow { .. } = action {
                let label = kuluu_render::hud::menu::entry_label(kind, cursor, dynamic);
                push_system_chat_line(scene_state, format!("[menu] {label} — not implemented"));
                return None;
            }

            // Bazaar stall picker select = “request to view a bazaar”: c2s 0x105
            // for that seller (vault main_menu.md §33), then kuluu's browse screen
            // takes over — the same seam as the /check wares path in check.rs.
            // The empty-list hint row never resolves to an entity and stays inert.
            if let A::BazaarStall { char_id } = action {
                let target_index = scene_state
                    .snapshot
                    .entities
                    .iter()
                    .find(|e| e.id == char_id && e.char_flags.bazaar)
                    .map(|e| e.act_index)?;
                if let Err(e) = cmd_tx.try_send(AgentCommand::OpenBazaar {
                    target_id: char_id,
                    target_index,
                }) {
                    push_system_chat_line(scene_state, format!("[menu] bazaar open dropped: {e}"));
                }
                return Some(InputMode::Bazaar);
            }

            // Craft-list select: the plan was resolved against the bag when the rows
            // were built, so this is that c2s 0x096 ask verbatim (vault main_menu.md
            // §28); the s2c 0x06F answer lands in snapshot.synth_result.
            if let A::SynthesizeRecipe {
                crystal_id,
                crystal_slot,
                ingredients,
            } = action
            {
                let ingredients = ingredients.into_iter().flatten().collect::<Vec<_>>();
                if let Err(e) = cmd_tx.try_send(AgentCommand::Synthesize {
                    crystal_id,
                    crystal_slot,
                    ingredients,
                }) {
                    push_system_chat_line(scene_state, format!("[menu] synth ask dropped: {e}"));
                }
                return None;
            }
            // Button row id 10 = Exit (vault main_menu.md §25): send Cancell and
            // close the window pair — kuluu pops back to Root, retail closes
            // both names through 0x15E910.
            if matches!(action, A::TradeCancelRow) {
                scene_state.snapshot.trade = None;
                if let Err(e) = cmd_tx.try_send(AgentCommand::TradeCancel) {
                    push_system_chat_line(scene_state, format!("[menu] trade cancel dropped: {e}"));
                }
                stack.pop();
                return None;
            }
            if let A::DropItem {
                container,
                index,
                item_no,
                quantity,
            } = action
            {
                stack.push(MenuKind::ItemDropConfirm {
                    container,
                    index,
                    item_no,
                    quantity,
                });
                if let Some(level) = stack.current_mut() {
                    level.cursor = kuluu_render::hud::menu::DROP_CONFIRM_DEFAULT;
                }
                return None;
            }
            // The macro family (bead kuluu-o9qt7.19.3): a store row drills its
            // line editor — retail's long-form chain from the palette/mcrselop
            // windows (FFXiMain.dll retail-2026-09 RVA 0x1A68C0/0x1A5690; vault
            // main_menu.md §34) — line rows seed the level's mcrledit buffer,
            // and the clear row commits without it. Nothing here touches the
            // wire except through CommitMacros (§41 [V-negative]).
            if let A::MacroSlotRow { block, index } = action {
                stack.push(MenuKind::MacroEditor { block, index });
                return None;
            }
            if let A::MacroEditLine { line_index } = action {
                let (block, index) = match kind {
                    MenuKind::MacroEditor { block, index } => (block, index),
                    _ => return None,
                };
                let seed = scene_state
                    .snapshot
                    .macros
                    .iter()
                    .find(|s| s.block == block && s.index == index)
                    .map(|s| kuluu_render::hud::menu::macro_slot_lines(&s.text))
                    .and_then(|ls| match line_index {
                        Some(i) => ls.get(i).map(|s| (*s).to_string()),
                        None => Some(String::new()),
                    })
                    .unwrap_or_default();
                if let Some(level) = stack.current_mut() {
                    level.macro_line_edit = Some(kuluu_render::input_mode::MacroLineBuffer {
                        block,
                        index,
                        line_no: line_index,
                        text: seed,
                    });
                }
                return None;
            }
            if let A::MacroClearSlot = action {
                let (block, index) = match kind {
                    MenuKind::MacroEditor { block, index } => (block, index),
                    _ => return None,
                };
                let slots: Vec<kuluu_snapshot::MacroSlot> = scene_state
                    .snapshot
                    .macros
                    .iter()
                    .filter(|s| !(s.block == block && s.index == index))
                    .cloned()
                    .collect();
                if let Err(e) = cmd_tx.try_send(AgentCommand::CommitMacros { slots }) {
                    push_system_chat_line(scene_state, format!("[menu] macro clear dropped: {e}"));
                }
                return None;
            }
            // Running a keytop (bead kuluu-o9qt7.19.4): the slot's command lines
            // become pending chat-pipeline steps — retail feeds a used macro one
            // line at a time into the same pipeline typed text takes (vault
            // main_menu.md §41/§56); no store write, no wire beyond what the
            // submitted lines themselves carry.
            if let A::MacroUse { block, index } = action {
                let steps = scene_state
                    .snapshot
                    .macros
                    .iter()
                    .find(|s| s.block == block && s.index == index)
                    .map(|s| {
                        kuluu_render::hud::menu::macro_slot_lines(&s.text)
                            .into_iter()
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                scene_state.queue_macro_steps(steps);
                return None;
            }
            if let A::UseItem {
                container, index, ..
            } = action
            {
                let snap = &scene_state.snapshot;
                let usable = kuluu_render::hud::item_meta::find_slot(snap, container, index)
                    .is_some_and(|slot| {
                        kuluu_render::hud::menu::item_usable_now(snap, slot)
                            && !kuluu_render::hud::item_meta::item_unusable(
                                slot,
                                kuluu_render::hud::item_meta::now_vana_ts(),
                            )
                    });
                if !usable {
                    return None;
                }
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
                // Retail's sub-target confirm step: the cursor always shows,
                // seeded on the current target when it passes the mask, else
                // self, else the nearest candidate (initial_candidate). Esc
                // restores this menu with its cursor intact.
                let return_to = InputMode::Menu(stack.clone());
                return open_sub_target(sub_action, target_id, scene_state, return_to);
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
                graphics.reselect_unequip,
            );
            // Retail's equip select case refreshes its windows in place instead of closing them
            // (FFXiMain.dll retail-2026-09 RVA 0x1322CE), so kuluu keeps the equip list up for
            // further swaps and keeps the bag open through a sort/move session; the
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
    if let MenuKind::PowerPrompt { shut_down } = kind {
        if label == kuluu_render::hud::menu::DROP_CONFIRM_YES {
            let (cmd, toast) = if shut_down {
                (
                    AgentCommand::ReqLogout {
                        kind: ReqLogoutKind::ShutdownToggle,
                    },
                    "[menu] Shut Down requested (~30s; instant in Mog House). \
                    `/shutdown off` to cancel.",
                )
            } else {
                (
                    AgentCommand::ReqLogout {
                        kind: ReqLogoutKind::LogoutToggle,
                    },
                    "[menu] Log Out requested (~30s; instant in Mog House). \
                    `/logout off` to cancel.",
                )
            };
            if let Err(e) = cmd_tx.try_send(cmd) {
                push_system_chat_line(scene_state, format!("[menu] dispatch dropped: {e}"));
            } else {
                push_system_chat_line(scene_state, toast.to_string());
            }
        }
        // No (or any other answer) just dismisses the prompt.
        stack.pop();
        return None;
    }
    if let MenuKind::RoomListConfirm { style } = kind {
        // The dialog answer lands here: Yes runs retail's yes-callback — the sender
        // wrapper .text FFXiMain.dll retail-2026-09 0x1D6AB0 → exactly one MYROOM_IS
        // Kind=Remodel with Param2 = this style word; No writes nothing (vendor/server/
        // src/map/packets/c2s/0x0cb_myroom_is.cpp is the only judge once it arrives).
        // Both answers dismiss the dialog back onto the roomlist, like every generic
        // confirm panel (bead kuluu-o9qt7.13.5).
        if label == kuluu_render::hud::menu::DROP_CONFIRM_YES
            && cmd_tx
                .try_send(AgentCommand::MogHouseRemodel { style })
                .is_err()
        {
            push_system_chat_line(
                scene_state,
                "[menu] mog house remodel send dropped".to_string(),
            );
        }
        stack.pop();
        return None;
    }
    if matches!(kind, MenuKind::KeyAssignResetPrompt) {
        // Retail's callback .text 0x196F30 gets the answer at [esp+8]: non-zero
        // tail-calls vtable slot +0x48 on both assignment panels — their factory
        // reset through wrapper FFXiMain.dll retail-2026-09 RVA 0x193830 (option ids
        // 0x4A..=0x69, then 0x6A..=0x89; vault main_menu.md §59 addendum 8). No touches
        // nothing. Client-local like every other config-store write — no wire here.
        if label == kuluu_render::hud::menu::DROP_CONFIRM_YES {
            for (option_id, value) in kuluu_render::hud::menu::key_assign_reset_defaults() {
                dynamic.config_option_values.insert(option_id, value);
            }
        }
        stack.pop();
        return None;
    }
    if let MenuKind::ItemDropConfirm {
        container,
        index,
        quantity,
        ..
    } = kind
    {
        if label == kuluu_render::hud::menu::DROP_CONFIRM_YES {
            if let Err(e) = cmd_tx.try_send(AgentCommand::DropItem {
                container,
                index,
                quantity,
            }) {
                push_system_chat_line(scene_state, format!("[menu] drop dropped: {e}"));
            }
        }
        stack.pop();
        stack.pop();
        return None;
    }
    // The myroom door row is identified by its select id (id 6 of handler FFXiMain.dll retail-2026-09
    // `.text 0x145460`), not by the words on it: while the house stands open the row wears the other
    // catalog caption, and a label-only match would strand it on "Not supported yet".
    let door_row = matches!(kind, MenuKind::HomeView)
        && cursor == kuluu_render::hud::menu::home_door_row_index();
    match if door_row {
        MenuDispatch::MogHouseDoor
    } else {
        resolve_menu_entry(
            kind,
            label,
            graphics.magic_flat_list,
            graphics.abilities_flat_list,
        )
    } {
        MenuDispatch::OpenMogMenu => {
            // The arm's veto: in a Mog-house capital without being inside your
            // own house it ends at the error sound (FFXiMain.dll retail-2026-09
            // RVA 0x14E8F5/0xF9680; vault main_menu.md §32) — kuluu's analog is
            // this hint line, same as the Trade gate above.
            if kuluu_render::hud::menu::view_house_blocked(
                scene_state.snapshot.zone_id,
                scene_state.snapshot.self_in_mog_house(),
            ) {
                push_system_chat_line(
                    scene_state,
                    "[menu] View House — not available from here".to_string(),
                );
                return None;
            }
            if let Err(e) = cmd_tx.try_send(AgentCommand::OpenMogMenu) {
                push_system_chat_line(scene_state, format!("[menu] Mog Menu open dropped: {e}"));
            }
            None
        }
        MenuDispatch::TextCommand(cmd) => {
            apply_menu_text_command(
                cmd,
                command_surface,
                stack,
                vana_clock,
                vana_clock_visible,
                scene_state,
                cmd_tx,
                self_pos,
                target_id,
                dynamic,
            );
            // Mirrors the Debug toggles: the menu stays open (provisional
            // pending retail capture, bead kuluu-y5hq retail_unknowns).
            None
        }
        MenuDispatch::HomeTextCommand(cmd) => {
            apply_menu_text_command(
                cmd,
                command_surface,
                stack,
                vana_clock,
                vana_clock_visible,
                scene_state,
                cmd_tx,
                self_pos,
                target_id,
                dynamic,
            );
            // The retail arms (FFXiMain.dll retail-2026-09 RVA 0x145541 and
            // 0x14554A) close myroom by name after the runner returns; kuluu
            // pops this window's level to do the same (vault §53).
            if !stack.pop() {
                return Some(InputMode::World);
            }
            None
        }
        MenuDispatch::BrowseBag { container } => {
            item_bag.0 = container;
            stack.push(MenuKind::Items);
            None
        }
        MenuDispatch::OpenDeliveryBox => {
            // Retail raises the `mogpost` chrome; kuluu's delivery screen is
            // server-owned, so the row opens the incoming box — the driver's
            // Work→Check sequence streams its list (kuluu-session/src/
            // delivery_box.rs mirrors retail's op order).
            if cmd_tx
                .try_send(AgentCommand::DeliveryBox {
                    op: kuluu_session::state::DeliveryBoxOp::PostOpen,
                })
                .is_err()
            {
                push_system_chat_line(scene_state, "[menu] delivery box open dropped".to_string());
            }
            None
        }
        MenuDispatch::MogHouseDoor => {
            // One packet per select, keeping the panel open as the retail arm does (it returns
            // without touching the window). The row's caption rides the DAT catalog pair with the
            // door byte, and Layout/Remodel go dim while the house is open — both in kuluu-render's
            // `entry_label` and `menu_row_disabled`.
            if cmd_tx.try_send(AgentCommand::MogHouseDoorSelect).is_err() {
                push_system_chat_line(
                    scene_state,
                    "[menu] mog house door select dropped".to_string(),
                );
            }
            None
        }
        MenuDispatch::OpenSubmenu(submenu) => {
            // The Root Equipment row obeys the variant law (FFXiMain.dll
            // retail-2026-09 RVA 0x1404D5: Monstrosity first via helper 0x141BA0
            // into `mp_mncst`, then the puppet flag + job byte 0x12 branch into
            // `pupequip`; vault main_menu.md §39). Data presence on the folded
            // snapshot stands in for both client flags (same law as
            // equipment_drill_kind's doc).
            let submenu =
                if matches!(submenu, MenuKind::Equipment) && matches!(kind, MenuKind::Root) {
                    kuluu_render::hud::menu::equipment_drill_kind(&scene_state.snapshot)
                } else if matches!(submenu, MenuKind::Map) && matches!(kind, MenuKind::Root) {
                    // The Root Map row obeys the myroom variant law (FFXiMain.dll
                    // retail-2026-09 RVA 0x140653: name-open `myroom` when in a
                    // Mog House, else drive the mapframe family via 0x1F7810(0);
                    // vault main_menu.md §40).
                    kuluu_render::hud::menu::map_drill_kind(&scene_state.snapshot)
                } else {
                    submenu
                };
            // The Root Trade row obeys helper 0x13FE70's gate law (vault
            // main_menu.md §25): it opens only against an eligible PC target and
            // refuses — with the error-sound arm at menuwind 0x1406AD, whose
            // kuluu analog is this hint line — when a trade already exists or no
            // valid target stands selected. Success puts c2s TRADE_REQ
            // {UniqueNo, ActIndex} on the wire; our windows open right away and
            // advance only once the partner answers (player_trade.cpp).
            if matches!(submenu, MenuKind::Trade) && matches!(kind, MenuKind::Root) {
                if scene_state.snapshot.trade.is_some() {
                    push_system_chat_line(
                        scene_state,
                        "[menu] Trade — already in a trade".to_string(),
                    );
                    return None;
                }
                let partner = scene_state.snapshot.entities.iter().find(|e| {
                    Some(e.id) == target_id
                        && e.kind == kuluu_snapshot::EntityKind::Pc
                        && Some(e.id) != scene_state.snapshot.self_char_id
                });
                match partner {
                    Some(entity) => {
                        if let Err(e) = cmd_tx.try_send(AgentCommand::TradeRequest {
                            char_id: entity.id,
                            act_index: entity.act_index,
                        }) {
                            push_system_chat_line(
                                scene_state,
                                format!("[menu] trade request dropped: {e}"),
                            );
                        }
                        stack.push(submenu);
                    }
                    None => {
                        push_system_chat_line(
                            scene_state,
                            "[menu] Trade — no valid target".to_string(),
                        );
                    }
                }
                return None;
            }
            // Refresh the job-emote/chair unlock bits whenever the Emote List
            // opens (c2s 0x119 → s2c 0x11A gates the Job row).
            if submenu == MenuKind::EmoteList {
                let _ = cmd_tx.try_send(AgentCommand::RequestEmoteList);
                // …and open the `abisortw` companion **first**, as retail's opener
                // `.text 0x225460` always does (bead kuluu-o9qt7.34): its re-bind pulls
                // any stale list down through stack dedupe, and the fresh push that
                // follows rebuilds it — retail's fill resets the swap state word and the
                // pending cursor on every one of those ([emote+0x6C]/[+0x70],
                // `.text 0x225960`), so kuluu clears its mirror here too.
                stack.push(MenuKind::AbilitySort { mode: 1 });
                dynamic.emote_sort_state = kuluu_render::hud::menu::EmoteSortState::Off;
            }
            // The power prompts open with the cursor on No, like every other
            // retail confirm.
            if matches!(submenu, MenuKind::PowerPrompt { .. }) {
                stack.open_confirm(submenu, ConfirmDefault::No);
                return None;
            }
            // The Map screen opens on its command submenu; the wide-scan request
            // (0x0F4) fires only when the player selects "Wide Scan", not on open.
            // `reset_map_screen_on_open` clears the submode as the screen appears.
            stack.push(submenu);
            None
        }
        MenuDispatch::NotImplemented(label) => {
            push_system_chat_line(scene_state, format!("[menu] {label} — not implemented"));
            None
        }
    }
}

/// Retail+ section rows (dev-only Debug menu). Returns true when the label is
/// one of them and it was handled here — the caller must not fall through to
/// `toggle_debug_panel`. The live toggles flip GraphicsSettings fields, so
/// `persist_graphics_on_change` writes graphics.json automatically. Mob HP
/// Under / Job Display only exist in enhanced builds (their rows are absent
/// from DEBUG_ENTRIES without their feature). The section-chrome rows
/// (separator + label) carry no state and answer with no banner. The DLSS
/// row reads N/A and its toggle is inert when this build can't run DLSS at
/// all (no dlss feature, or no RTX/Vulkan/DLLs), so the persisted gate does
/// not flip.
fn handle_retail_plus_row(
    label: &str,
    graphics: &mut kuluu_render::GraphicsSettings,
    scene_state: &mut SceneState,
) -> bool {
    #[cfg(feature = "enhanced-job-display")]
    use kuluu_render::hud::menu::RETAIL_JOB_DISPLAY;
    #[cfg(feature = "enhanced-ignore-knockback-self")]
    use kuluu_render::hud::menu::RETAIL_KNOCKBACK;
    #[cfg(feature = "enhanced-mob-hp-under")]
    use kuluu_render::hud::menu::RETAIL_MOB_HP_UNDER;
    use kuluu_render::hud::menu::{
        DEBUG_RETAIL_LABEL, DEBUG_RETAIL_SEPARATOR, RETAIL_ABILITIES_FLAT_LIST,
        RETAIL_CAMERA_LEASH, RETAIL_DLSS_MENU, RETAIL_MAGIC_FLAT_LIST, RETAIL_RESELECT_UNEQUIP,
        RETAIL_STATUS_ROWS,
    };
    match label {
        DEBUG_RETAIL_SEPARATOR | DEBUG_RETAIL_LABEL => true,
        RETAIL_STATUS_ROWS => {
            graphics.interactive_status_rows = !graphics.interactive_status_rows;
            push_system_chat_line(
                scene_state,
                format!(
                    "[menu] {label}: {}",
                    if graphics.interactive_status_rows {
                        "on"
                    } else {
                        "off"
                    }
                ),
            );
            true
        }
        // Retail's equip handler equips on every select case (FFXiMain.dll retail-2026-09 RVA
        // 0x1322B0, vault main_menu.md §20); the unequip-on-reselect shortcut is opt-in here.
        RETAIL_RESELECT_UNEQUIP => {
            graphics.reselect_unequip = !graphics.reselect_unequip;
            push_system_chat_line(
                scene_state,
                format!(
                    "[menu] {label}: {}",
                    if graphics.reselect_unequip {
                        "on"
                    } else {
                        "off"
                    }
                ),
            );
            true
        }
        // Retail's Magic row opens mgcmenu, whose handler turns a category pick into the
        // `magic` window's school filter (FFXiMain.dll retail-2026-09 RVA 0x13F750,
        // jump table 0x13F7CC; vault main_menu.md §21); kuluu's flat one-step spell list is
        // the opt-in enhancement.
        RETAIL_MAGIC_FLAT_LIST => {
            graphics.magic_flat_list = !graphics.magic_flat_list;
            push_system_chat_line(
                scene_state,
                format!(
                    "[menu] {label}: {}",
                    if graphics.magic_flat_list {
                        "on"
                    } else {
                        "off"
                    }
                ),
            );
            true
        }
        // Retail's Abilities row opens abimenu, whose rows filter the `ability`
        // list (row 6 opening `menu    mount`); kuluu's flat one-step ability list
        // is the opt-in enhancement (FFXiMain.dll retail-2026-09 RVA 0x13FA70,
        // vault main_menu.md §23).
        RETAIL_ABILITIES_FLAT_LIST => {
            graphics.abilities_flat_list = !graphics.abilities_flat_list;
            push_system_chat_line(
                scene_state,
                format!(
                    "[menu] {label}: {}",
                    if graphics.abilities_flat_list {
                        "on"
                    } else {
                        "off"
                    }
                ),
            );
            true
        }
        // Number row: the value moves with Left/Right, so confirm does
        // nothing rather than fall through to toggle_debug_panel.
        RETAIL_CAMERA_LEASH => true,
        RETAIL_DLSS_MENU => {
            if !graphics.dlss_supported {
                push_system_chat_line(
                    scene_state,
                    format!("[menu] {label}: N/A (this build can't run DLSS)"),
                );
                true
            } else {
                graphics.dlss_menu_enabled = !graphics.dlss_menu_enabled;
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
        #[cfg(feature = "enhanced-ignore-knockback-self")]
        RETAIL_KNOCKBACK => {
            // [on] reports the feature live: knockback applied (the default).
            graphics.ignore_knockback_self = !graphics.ignore_knockback_self;
            push_system_chat_line(
                scene_state,
                format!(
                    "[menu] {label}: {}",
                    if graphics.ignore_knockback_self {
                        "off"
                    } else {
                        "on"
                    }
                ),
            );
            true
        }
        _ => false,
    }
}

/// Flip a Debug-menu toggle and report it. The weather/fog rows report the
/// feature's live state, so they invert the off flags: Weather [on] = weather
/// effects applied.
fn toggle_debug_panel(
    label: &str,
    hud_panels: &mut kuluu_render::hud::HudPanels,
    net_status: &mut kuluu_render::hud::network_status::NetStatusVisible,
    audio_mute: &mut kuluu_render::audio::AudioMuteState,
    vana_clock: &mut kuluu_render::vana_time::VanaClock,
    self_pos: kuluu_snapshot::Vec3,
    scene_state: &mut SceneState,
) {
    #[cfg(feature = "enhanced-engage-move-lock-off")]
    use kuluu_render::hud::menu::DEBUG_ENGAGE_ANIM_LOCK;
    use kuluu_render::hud::menu::{
        DEBUG_AUTO_ENTER_CS, DEBUG_ENTITY_LIST, DEBUG_FOG, DEBUG_FORCE_18, DEBUG_GRAPHICS_DEBUG,
        DEBUG_MESH, DEBUG_NAMEPLATES, DEBUG_NET_STATUS, DEBUG_NOCLIP, DEBUG_PERF,
        DEBUG_POSITION_LOG, DEBUG_PRINT_POS, DEBUG_SOUND, DEBUG_STAIR_DRAW, DEBUG_STAIR_STATUS,
        DEBUG_TARGET_CYCLE, DEBUG_UI_SETTINGS, DEBUG_WEATHER,
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
        #[cfg(feature = "enhanced-engage-move-lock-off")]
        DEBUG_ENGAGE_ANIM_LOCK => {
            hud_panels.engage_anim_lock = !hud_panels.engage_anim_lock;
            hud_panels.engage_anim_lock
        }
        DEBUG_AUTO_ENTER_CS => {
            hud_panels.auto_enter_cs = !hud_panels.auto_enter_cs;
            hud_panels.auto_enter_cs
        }
        DEBUG_WEATHER => {
            hud_panels.weather_off = !hud_panels.weather_off;
            !hud_panels.weather_off
        }
        DEBUG_FOG => {
            hud_panels.fog_off = !hud_panels.fog_off;
            !hud_panels.fog_off
        }
        DEBUG_FORCE_18 => {
            // Activation takes over another owner's hold; deactivation thaws
            // whatever the row froze.
            if vana_clock.debug_hold_active() {
                vana_clock.thaw();
                false
            } else {
                const FORCE_VANA_HOUR: u32 = 18;
                vana_clock.freeze_at_hour_minute(
                    FORCE_VANA_HOUR,
                    0,
                    kuluu_render::vana_time::ClockHoldOrigin::DebugRow,
                );
                true
            }
        }
        DEBUG_ENTITY_LIST => {
            hud_panels.entity_list = !hud_panels.entity_list;
            hud_panels.entity_list
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

/// Consume one keystroke while a macro line editor buffer is open (bead
/// kuluu-o9qt7.19.3): printable characters append, Backspace pops, Enter
/// commits the merged set through CommitMacros (retail's mcrledit commit —
/// FFXiMain.dll retail-2026-09 RVA 0x1A6180 runs its blip then stores;
/// vault main_menu.md §34/§41), Esc discards. The menu never closes here.
fn macro_line_edit_key(
    key: &Key,
    bindings: &Bindings,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) {
    if bindings.matches_logical(Action::NavCancel, key) {
        if let Some(level) = stack.current_mut() {
            level.macro_line_edit = None;
        }
        return;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        let edit = stack
            .current()
            .and_then(|l| l.macro_line_edit.as_ref())
            .map(|b| (b.block, b.index, b.line_no, b.text.clone()));
        if let Some((block, index, line_no, text)) = edit {
            let slots = kuluu_render::hud::menu::merge_macro_line(
                &scene_state.snapshot.macros,
                block,
                index,
                line_no,
                &text,
            );
            if let Err(e) = cmd_tx.try_send(AgentCommand::CommitMacros { slots }) {
                push_system_chat_line(scene_state, format!("[menu] macro commit dropped: {e}"));
            }
        }
        if let Some(level) = stack.current_mut() {
            level.macro_line_edit = None;
        }
        return;
    }
    let Some(buf) = stack.current_mut().and_then(|l| l.macro_line_edit.as_mut()) else {
        return;
    };
    if bindings.matches_logical(Action::ChatBackspace, key) {
        buf.text.pop();
        return;
    }
    match key {
        Key::Space => buf.text.push(' '),
        Key::Character(s) => {
            for c in s.chars() {
                if !c.is_control() {
                    buf.text.push(c);
                }
            }
        }
        _ => {}
    }
}

/// Route one keypress in the open menu. In the sort pane, any key the pane
/// does not own maps to SortPaneKey::Other so it can't leak into list
/// navigation. Commands > Items opens on the inventory; only the Mog Menu
/// storage rows open the window on another bag.
pub(super) fn handle_menu_key(
    key: &Key,
    key_code: KeyCode,
    bindings: &mut Bindings,
    stack: &mut MenuStack,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    keybinds_state: &mut KeybindsStateRes,
    graphics: &mut kuluu_render::GraphicsSettings,
    hud_panels: &mut kuluu_render::hud::HudPanels,
    net_status: &mut kuluu_render::hud::network_status::NetStatusVisible,
    audio_mute: &mut kuluu_render::audio::AudioMuteState,
    vana_clock: &mut kuluu_render::vana_time::VanaClock,
    vana_clock_visible: &mut kuluu_render::hud::vana_clock::VanaClockVisible,
    sort_options: &mut kuluu_render::hud::item_detail::SortOptions,
    item_menu_focus: &mut kuluu_render::hud::item_detail::ItemMenuFocus,
    item_bag: &mut kuluu_render::hud::item_screen::ItemScreenContainer,
    item_viewport: &mut kuluu_render::hud::item_screen::ItemListViewport,
    dynamic: &mut kuluu_render::hud::menu::DynamicMenu,
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
    change_map_catalog: &kuluu_render::hud::map_screen::ChangeMapCatalog,
    command_surface: &crate::view_native::command_surface::CommandSurface,
) -> Option<InputMode> {
    let top_kind = stack.current()?.kind;

    // The very key that opened this menu (Action::OpenMenu on "-", handled by the
    // input system chained just before this one) also arrives here on the same
    // frame; absorb it once so it doesn't immediately flip Root to page 2
    // (kuluu-bi1s.2). Clear the one-shot flag on the first menu key regardless.
    if stack.take_absorb_open_minus() && key_code == KeyCode::Minus {
        return None;
    }
    // .agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md: a window
    // whose cursor rests on its unselectable last item ignores every key, Esc included.
    if stack
        .current()
        .is_some_and(kuluu_render::input_mode::MenuLevel::cursor_locked)
    {
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
            change_map_catalog,
        );
    }
    let (kind, cursor) = {
        let level = stack.current()?;
        (level.kind, level.cursor)
    };
    let entry_count = kuluu_render::hud::menu::entry_count(kind, dynamic, graphics.dlss_supported);

    // Menu context (not text input), so reading the raw keycode is correct.
    // "-" advances the Command menu's pages (retail HorizonXI flips two; kuluu
    // cycles three); single-list submenus have no pages, and the Map screen
    // handles "-" in its own path.
    if key_code == KeyCode::Minus {
        if kind == MenuKind::Root {
            if let Some(level) = stack.current_mut() {
                level.cursor = kuluu_render::hud::menu::root_next_page_cursor(level.cursor);
            }
        }
        return None;
    }

    // statcom2 focus law (bead kuluu-0oy7b, vault main_menu.md §59): a display-only
    // detail companion never takes the keys — Up/Down keep moving the Status
    // selector underneath and Enter re-dispatches its current row.
    if let Some(sel) = statcom_selector_below(stack) {
        let sel_cursor = stack.levels[sel].cursor;
        let sel_count = kuluu_render::hud::menu::entry_count(
            MenuKind::Status,
            dynamic,
            graphics.dlss_supported,
        );
        if bindings.matches_logical(Action::NavUp, key) {
            let up = if sel_cursor == 0 {
                sel_count.saturating_sub(1)
            } else {
                sel_cursor - 1
            };
            stack.levels[sel].cursor = kuluu_render::hud::menu::settle_cursor(
                MenuKind::Status,
                graphics.dlss_supported,
                up,
                false,
            );
            return None;
        }
        if bindings.matches_logical(Action::NavDown, key) {
            let next = sel_cursor + 1;
            let down = if next >= sel_count { 0 } else { next };
            stack.levels[sel].cursor = kuluu_render::hud::menu::settle_cursor(
                MenuKind::Status,
                graphics.dlss_supported,
                down,
                true,
            );
            return None;
        }
        if bindings.matches_logical(Action::NavConfirm, key) {
            use kuluu_render::hud::status_panel::{StatusEntryKind, STATUS_ENTRIES};
            if !graphics.interactive_status_rows {
                return None;
            }
            match STATUS_ENTRIES.get(sel_cursor).map(|e| e.kind) {
                Some(StatusEntryKind::PlayTime) => {
                    let line =
                        kuluu_render::hud::status_panel::play_time_chat_line(&scene_state.snapshot);
                    push_system_chat_line(scene_state, line);
                }
                // Same enable law as the top-of-stack path: a greyed row never
                // reaches the detail table (FFXiMain.dll retail-2026-09 `.text 0x148B50`).
                Some(entry_kind)
                    if kuluu_render::hud::status_panel::status_row_enabled(
                        entry_kind,
                        &scene_state.snapshot,
                    ) =>
                {
                    if let Some(mode) = currency_request_mode_for(entry_kind) {
                        let _ = cmd_tx.try_send(AgentCommand::RequestCurrencies { mode });
                    }
                    open_status_row(stack, sel_cursor, scene_state, cmd_tx, dynamic);
                }
                // Guard failed: the row is grey, so the select is inert.
                _ => {}
            }
            return None;
        }
    }

    // mcrledit line editor (bead kuluu-o9qt7.19.3): while the window holds an
    // edit buffer every keystroke belongs to it — retail's macro line editor
    // owns input until Enter commits or Esc discards (FFXiMain.dll
    // retail-2026-09 RVA 0x1A6180; vault main_menu.md §34/§41). Nav keys stay
    // dead while typing, which is also the `.29` rule.
    if stack.current().is_some_and(|l| l.macro_line_edit.is_some()) {
        macro_line_edit_key(key, bindings, stack, scene_state, cmd_tx);
        return None;
    }

    // conf2win slider-pin law (bead kuluu-o9qt7.20.1 part B): while a slider is
    // pinned, every window event routes to that window's key table — Up/Down are
    // arms 0/1 and do nothing at all (the cursor cannot move), Left steps −1 only
    // above 0 and Right +1 only below 100 (`test ecx/jle` and `cmp ecx,0x64/jge`
    // guards), each step writing the store immediately through wrapper
    // FFXiMain.dll retail-2026-09 RVA 0x193830, and Enter/Esc (arms 4/5 — one
    // shared save-and-leave body) rewrite the store once more and unpin. Any
    // other event hits the handler's `ja ret` tail: swallowed. Key tables .text
    // 0x197874/0x19788C of conf2win handler RVA 0x197470 (vault main_menu.md
    // §59 addendum 2). A pin only lives on its own window — any other top kind
    // drops it.
    // The same pin law carries to the Config sub-windows that own steppers — the two
    // individual log-window editors, whose per-store step/cross-clamp extras the guards
    // below read from the shared law helpers (vault main_menu.md §59 addendum 46).
    let slider_page = match top_kind {
        MenuKind::RetailConfigPage(page) => kuluu_render::hud::menu::RETAIL_CONFIG_PAGE_WINDOWS
            .get(usize::from(page))
            .is_some_and(|w| kuluu_render::hud::menu::config_page_supports_slider_pin(w)),
        MenuKind::RetailConfigSub(sub) => kuluu_render::hud::menu::RETAIL_CONFIG_SUB_WINDOWS
            .get(usize::from(sub))
            .is_some_and(|w| kuluu_render::hud::menu::config_page_supports_slider_pin(w)),
        _ => false,
    };
    if !slider_page {
        dynamic.option_edit = None;
    } else if let Some((store_id, value)) = dynamic.option_edit {
        let step = kuluu_render::hud::menu::config_slider_step(store_id);
        if bindings.matches_logical(Action::NavLeft, key) {
            // `conf3win`'s step arms also carry a floor (`cmp eax,0x19/jle`,
            // FFXiMain.dll retail-2026-09 RVA .text 0x19ABDB), so the decrement
            // guard is per-store rather than always zero. The log-window width arm
            // steps by 10 and clamps after subtracting (.text 0x199A4B/0x199A55), so
            // the same clamp-after-arithmetic shape covers every stepper.
            let floor = i32::from(kuluu_render::hud::menu::config_slider_min(store_id));
            if i32::from(value) > floor {
                let next = (i32::from(value) - step).max(floor);
                dynamic.config_option_values.insert(store_id, next);
                let next = next as u8;
                dynamic.option_edit = Some((store_id, next));
                // The max-lines decrement arm re-reads the min-lines store and lowers
                // it to the new maximum when it exceeds it (.text 0x199AEF..0x199B15).
                if let Some((max_store, min_store)) =
                    kuluu_render::hud::menu::config_log_lines_pair(store_id)
                {
                    if store_id == max_store
                        && dynamic
                            .config_option_values
                            .get(&min_store)
                            .is_some_and(|min_value| *min_value > i32::from(next))
                    {
                        dynamic
                            .config_option_values
                            .insert(min_store, i32::from(next));
                    }
                }
            }
        } else if bindings.matches_logical(Action::NavRight, key) {
            let ceiling = i32::from(kuluu_render::hud::menu::config_slider_max(store_id));
            if i32::from(value) < ceiling {
                let next = (i32::from(value) + step).min(ceiling);
                dynamic.config_option_values.insert(store_id, next);
                let next = next as u8;
                dynamic.option_edit = Some((store_id, next));
                // The min-lines increment arm raises max-lines to the new minimum
                // when it crosses it (.text 0x199BBF..0x199BE8).
                if let Some((max_store, min_store)) =
                    kuluu_render::hud::menu::config_log_lines_pair(store_id)
                {
                    if store_id == min_store
                        && dynamic
                            .config_option_values
                            .get(&max_store)
                            .is_some_and(|max_value| *max_value < i32::from(next))
                    {
                        dynamic
                            .config_option_values
                            .insert(max_store, i32::from(next));
                    }
                }
            }
        } else if bindings.matches_logical(Action::NavUp, key)
            || bindings.matches_logical(Action::NavDown, key)
        {
            // A pinned slider never silences vertical travel. Retail reads every direction
            // from its own neighbour byte (window input switch FFXiMain.dll retail-2026-09
            // RVA 0x118260; bead kuluu-8l5h4), so Up/Down drop the pin and move rows just as
            // they would on an unpinned row. The store keeps whatever was stepped so far.
            let down = bindings.matches_logical(Action::NavDown, key);
            dynamic.option_edit = None;
            if let Some(level) = stack.current_mut() {
                level.step_cursor_to(kuluu_render::hud::list_view::step_cursor(
                    level.cursor,
                    dynamic.rows.len(),
                    down,
                ));
            }
            return None;
        } else if bindings.matches_logical(Action::NavConfirm, key)
            || bindings.matches_logical(Action::NavCancel, key)
        {
            // Save-and-leave: the store already holds each stepped value; the
            // exit arm writes it once more for identical effect, then unpins.
            dynamic
                .config_option_values
                .insert(store_id, i32::from(value));
            dynamic.option_edit = None;
        }
        return None;
    }

    // Root Command menu paging: Right advances like "-", Left steps back through
    // the pages; Up/Down wrap within the current page so row navigation never
    // crosses a page boundary.
    if kind == MenuKind::Root {
        let (start, end) = kuluu_render::hud::menu::root_page_bounds(cursor);
        if bindings.matches_logical(Action::NavRight, key) {
            if let Some(level) = stack.current_mut() {
                level.cursor = kuluu_render::hud::menu::root_next_page_cursor(level.cursor);
            }
            return None;
        }
        if bindings.matches_logical(Action::NavLeft, key) {
            if let Some(level) = stack.current_mut() {
                level.cursor = kuluu_render::hud::menu::root_prev_page_cursor(level.cursor);
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
    // Options box. Retail's "Select active window" key (Numpad + on the full
    // keyboard) steps focus through them; Up/Down clamp at both ends and
    // Left/Right shift cursor and page together by a full row count
    // (.agents/skills/retail-observe/references/2026-09-11-items-window.md).
    if matches!(kind, MenuKind::Items | MenuKind::UsableItems) {
        use kuluu_render::hud::item_detail::{sort_pane_key, SortOptionId, SortPaneKey};
        use kuluu_render::hud::item_screen::{page_cursor, step_cursor};
        if kind == MenuKind::Items {
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
                    SortPaneKey::Other
                };
                match sort_pane_key(item_menu_focus, sort_options, pane_key) {
                    Some(SortOptionId::RecycleBin) => {
                        if kuluu_render::hud::item_screen::open_recycle_bin(
                            &scene_state.snapshot,
                            item_bag,
                            item_menu_focus,
                        ) {
                            if let Some(level) = stack.current_mut() {
                                level.cursor = 0;
                            }
                        }
                    }
                    Some(_) => {
                        if let Err(e) = cmd_tx.try_send(AgentCommand::StackInventory {
                            container: ffxi_proto::map::container::LOC_INVENTORY,
                        }) {
                            push_system_chat_line(
                                scene_state,
                                format!("sort dropped (channel): {e}"),
                            );
                        }
                    }
                    None => {}
                }
                return None;
            }
        }
        let page = if bindings.matches_logical(Action::NavLeft, key) {
            Some(false)
        } else if bindings.matches_logical(Action::NavRight, key) {
            Some(true)
        } else {
            None
        };
        if let Some(forward) = page {
            item_viewport.page(forward, entry_count);
            if let Some(level) = stack.current_mut() {
                level.cursor = page_cursor(level.cursor, entry_count, forward);
            }
            return None;
        }
        let step = if bindings.matches_logical(Action::NavUp, key) {
            Some(false)
        } else if bindings.matches_logical(Action::NavDown, key) {
            Some(true)
        } else {
            None
        };
        if let Some(down) = step {
            if let Some(level) = stack.current_mut() {
                level.cursor = step_cursor(level.cursor, entry_count, down);
            }
            return None;
        }
    }

    if matches!(
        kind,
        MenuKind::Config | MenuKind::Graphics | MenuKind::GraphicsDlss
    ) {
        if bindings.matches_logical(Action::NavLeft, key) {
            apply_settings_cycle(kind, cursor, -1, graphics);
            return None;
        }
        if bindings.matches_logical(Action::NavRight, key) {
            apply_settings_cycle(kind, cursor, 1, graphics);
            return None;
        }
    }

    // Debug menu: the Volume and Camera_leash rows are numbers adjusted with
    // Left/Right. Every other Debug row is a toggle handled on the confirm
    // key, so only those two rows consume arrows here; anything else falls
    // through to normal list navigation.
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
        if label == kuluu_render::hud::menu::RETAIL_CAMERA_LEASH {
            // A stepper owns Left/Right ONLY (bead kuluu-o9qt7.29): Up, Down,
            // Enter and Esc must keep reaching the generic handlers while the
            // cursor sits on this row — in retail every direction reads its own
            // neighbor byte, so a horizontal control can never silence vertical
            // travel (window input switch FFXiMain.dll retail-2026-09 RVA 0x118260;
            // vault main_menu.md §5 nav law).
            let left = bindings.matches_logical(Action::NavLeft, key);
            let right = bindings.matches_logical(Action::NavRight, key);
            if !left && !right {
                // Not a stepper keystroke: fall through to list navigation.
            } else {
                let units = (graphics.camera_leash_yalms
                    / kuluu_render::graphics_settings::CAMERA_LEASH_STEP_YALMS)
                    .round() as i32
                    + if left { -1 } else { 1 };
                graphics.camera_leash_yalms = (units.max(0) as f32
                    * kuluu_render::graphics_settings::CAMERA_LEASH_STEP_YALMS)
                    .min(kuluu_render::graphics_settings::CAMERA_LEASH_MAX_YALMS);
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

    // Left/Right toggle `jbpcat`'s page
    // (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
    if (bindings.matches_logical(Action::NavLeft, key)
        || bindings.matches_logical(Action::NavRight, key))
        && kuluu_render::hud::merit_windows::toggle_job_page(stack, dynamic)
    {
        return None;
    }

    // The ranking list's Left/Right switch result sets once both are complete
    // (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    if matches!(kind, MenuKind::UnityRanking { .. })
        && (bindings.matches_logical(Action::NavLeft, key)
            || bindings.matches_logical(Action::NavRight, key))
        && kuluu_render::hud::unity_fp::switch_ranking_set(stack, dynamic, &scene_state.snapshot)
    {
        return None;
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
            level.step_cursor_to(kuluu_render::hud::menu::page_cursor(
                level.cursor,
                entry_count,
                rows,
                forward,
            ));
            return None;
        }
    }

    if bindings.matches_logical(Action::NavUp, key) {
        let level = stack.current_mut()?;
        let up = if cursor == 0 {
            entry_count.saturating_sub(1)
        } else {
            cursor - 1
        };
        level.step_cursor_to(kuluu_render::hud::menu::settle_cursor(
            kind,
            graphics.dlss_supported,
            up,
            false,
        ));
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        let level = stack.current_mut()?;
        let next = cursor + 1;
        let down = if next >= entry_count { 0 } else { next };
        level.step_cursor_to(kuluu_render::hud::menu::settle_cursor(
            kind,
            graphics.dlss_supported,
            down,
            true,
        ));
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        let next = confirm_menu_at_cursor(
            bindings,
            stack,
            scene_state,
            cmd_tx,
            keybinds_state,
            graphics,
            hud_panels,
            net_status,
            audio_mute,
            vana_clock,
            vana_clock_visible,
            dynamic,
            target_id,
            self_pos,
            command_surface,
            item_bag,
        );
        if kind == MenuKind::Root && stack.current().is_some_and(|l| l.kind == MenuKind::Items) {
            item_bag.0 = ffxi_proto::map::container::LOC_INVENTORY;
        }
        return next;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        // statcom2 carries registry flag 0x2 so Esc reaches it: with a
        // display-only detail up the handler's close-details step fires first;
        // closing the selector cascades over its companions (statcom2's close
        // code — FFXiMain.dll retail-2026-09 RVA 0x148BB0 — closes persona,
        // whose close closes level; vault main_menu.md §59). Kuluu mirrors the
        // cascade by popping one stack level per Esc.
        // The emote swap's cancel arm fires before any close (bead kuluu-o9qt7.34):
        // while `[emote+0x6C]` is pending, Esc resets the state word and clears the
        // prompt line — the window stays open; only a settled list closes
        // (FFXiMain.dll retail-2026-09 `.text 0x225E34`).
        if kind == MenuKind::EmoteList
            && dynamic.emote_sort_state != kuluu_render::hud::menu::EmoteSortState::Off
        {
            dynamic.emote_sort_state = kuluu_render::hud::menu::EmoteSortState::Off;
            return None;
        }
        if kuluu_render::hud::merit_windows::cancel_merit_window(stack, dynamic) {
            return None;
        }
        if let Some(next) = kuluu_render::myroom_arrange::cancel_arrange_window(stack, dynamic) {
            return next;
        }
        // Cancel pops one level; from Root it closes back to the world. Closing the
        // emote list cascades over its `abisortw` companion — retail's close handler
        // closes abisortw unconditionally (.text 0x225A88, via CloseMenu 0x15E910) —
        // so kuluu pops the companion level straight behind it.
        let cascade_companion = kind == MenuKind::EmoteList;
        return if !stack.pop() {
            Some(InputMode::World)
        } else {
            if cascade_companion
                && stack
                    .current()
                    .is_some_and(|l| matches!(l.kind, MenuKind::AbilitySort { .. }))
            {
                stack.pop();
            }
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
    use kuluu_render::hud::map_screen::{
        ChangeMapCatalog, MapMarkers, MapScreenState, MapSubMode, MapView,
    };
    use kuluu_render::input_mode::Pane;
    use kuluu_render::minimap::{MinimapAabb, MinimapState};

    struct Harness {
        bindings: Bindings,
        scene_state: SceneState,
        keybinds_state: KeybindsStateRes,
        graphics: kuluu_render::GraphicsSettings,
        hud_panels: kuluu_render::hud::HudPanels,
        net_status: kuluu_render::hud::network_status::NetStatusVisible,
        audio_mute: kuluu_render::audio::AudioMuteState,
        vana_clock: kuluu_render::vana_time::VanaClock,
        vana_clock_visible: kuluu_render::hud::vana_clock::VanaClockVisible,
        sort_options: kuluu_render::hud::item_detail::SortOptions,
        item_menu_focus: kuluu_render::hud::item_detail::ItemMenuFocus,
        item_bag: kuluu_render::hud::item_screen::ItemScreenContainer,
        item_viewport: kuluu_render::hud::item_screen::ItemListViewport,
        dynamic: kuluu_render::hud::menu::DynamicMenu,
        map_state: MapScreenState,
        map_view: MapView,
        minimap_state: MinimapState,
        change_map_catalog: ChangeMapCatalog,
        command_surface: crate::view_native::command_surface::CommandSurface,
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
                hud_panels: Default::default(),
                net_status: Default::default(),
                audio_mute: Default::default(),
                vana_clock: kuluu_render::vana_time::VanaClock::anchored_at_hour(12.0),
                vana_clock_visible: Default::default(),
                sort_options: Default::default(),
                item_menu_focus: Default::default(),
                item_bag: Default::default(),
                item_viewport: Default::default(),
                dynamic: Default::default(),
                map_state: MapScreenState::default(),
                map_view: MapView::default(),
                minimap_state: MinimapState::default(),
                change_map_catalog: ChangeMapCatalog::default(),
                command_surface: crate::view_native::command_surface::CommandSurface::default(),
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
                &mut self.hud_panels,
                &mut self.net_status,
                &mut self.audio_mute,
                &mut self.vana_clock,
                &mut self.vana_clock_visible,
                &mut self.sort_options,
                &mut self.item_menu_focus,
                &mut self.item_bag,
                &mut self.item_viewport,
                &mut self.dynamic,
                None,
                kuluu_snapshot::Vec3::default(),
                &mut self.map_state,
                map_markers,
                &self.map_view,
                &self.minimap_state,
                &self.change_map_catalog,
                &self.command_surface,
            )
        }
    }

    fn marker_world() -> World {
        let mut world = World::new();
        world.insert_resource(MapMarkers::default());
        world.clear_trackers();
        world
    }

    /// Row 0 of Config is the "Interface" section header, so the cursor
    /// settles onto the first selectable row below it.
    #[test]
    fn config_keys_cycle_radar_and_open_controls_without_changing_bindings() {
        use kuluu_render::graphics_settings::config_rows;
        use kuluu_render::graphics_settings::{MenuAction, MenuRow};
        use kuluu_render::hud::menu::settle_cursor;
        use kuluu_render::MinimapRadar;
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Config);
        let minimap_slot = settle_cursor(MenuKind::Config, false, 0, true);
        stack.current_mut().unwrap().cursor = minimap_slot;
        for (key, code, expected) in [
            (Key::ArrowRight, KeyCode::ArrowRight, MinimapRadar::Enhanced),
            (Key::ArrowLeft, KeyCode::ArrowLeft, MinimapRadar::Vanilla),
            (Key::Enter, KeyCode::Enter, MinimapRadar::Enhanced),
        ] {
            harness.key(&key, code, &mut stack, world.resource_mut::<MapMarkers>());
            assert_eq!(harness.graphics.minimap_radar, expected);
            assert_eq!(stack.current().unwrap().cursor, minimap_slot);
            assert_eq!(stack.current().unwrap().kind, MenuKind::Config);
        }
        let controls_slot = config_rows()
            .iter()
            .position(|r| *r == MenuRow::Action(MenuAction::Controls))
            .expect("Controls row");
        stack.current_mut().unwrap().cursor = controls_slot;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.current().unwrap().kind, MenuKind::Controls);
        // …and the page-3 Controls list is live now (bead kuluu-o9qt7.26.1): its
        // command rows carry their actions instead of resolving by label.
        let rows = kuluu_render::hud::menu::controls_rows(
            &kuluu_render::keybinds::Preset::Standard.bindings(),
        );
        let compact_row = rows
            .iter()
            .find(|r| r.label == "Compact 1")
            .expect("preset command row");
        assert_eq!(
            compact_row.action,
            kuluu_render::hud::menu::DynamicMenuAction::ControlsPreset(
                kuluu_render::keybinds::Preset::Compact1,
            )
        );
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

    /// The clock is the only truth for the Force_18 row: activation freezes
    /// with a debug-row origin (taking over another owner's hold), deactivation
    /// thaws, and `VanaClock::debug_hold_active` alone drives the row.
    #[test]
    fn force_18_row_reads_and_writes_only_the_clock() {
        use kuluu_render::hud::menu::DEBUG_FORCE_18;
        use kuluu_render::vana_time::{ClockHoldOrigin, VanaClock};
        let mut hud_panels = kuluu_render::hud::HudPanels::default();
        let mut net_status = kuluu_render::hud::network_status::NetStatusVisible(false);
        let mut audio_mute = kuluu_render::audio::AudioMuteState::default();
        let mut scene_state = SceneState::default();
        let self_pos = kuluu_snapshot::Vec3::default();
        let mut clock = VanaClock::default();

        toggle_debug_panel(
            DEBUG_FORCE_18,
            &mut hud_panels,
            &mut net_status,
            &mut audio_mute,
            &mut clock,
            self_pos,
            &mut scene_state,
        );
        assert!(clock.is_frozen());
        assert_eq!(clock.hold_origin(), Some(ClockHoldOrigin::DebugRow));

        toggle_debug_panel(
            DEBUG_FORCE_18,
            &mut hud_panels,
            &mut net_status,
            &mut audio_mute,
            &mut clock,
            self_pos,
            &mut scene_state,
        );
        assert!(!clock.is_frozen());
        assert_eq!(clock.hold_origin(), None);

        // A foreign hold reads as off and is taken over; toggling then thaws.
        clock.freeze(ClockHoldOrigin::Cutscene);
        assert!(!clock.debug_hold_active());
        toggle_debug_panel(
            DEBUG_FORCE_18,
            &mut hud_panels,
            &mut net_status,
            &mut audio_mute,
            &mut clock,
            self_pos,
            &mut scene_state,
        );
        assert_eq!(clock.hold_origin(), Some(ClockHoldOrigin::DebugRow));
        toggle_debug_panel(
            DEBUG_FORCE_18,
            &mut hud_panels,
            &mut net_status,
            &mut audio_mute,
            &mut clock,
            self_pos,
            &mut scene_state,
        );
        assert!(!clock.is_frozen());
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

    #[test]
    fn power_prompt_opens_on_no_then_yes_only_requests_on_yes() {
        use kuluu_render::hud::menu::DROP_CONFIRM_DEFAULT;

        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.current_mut().unwrap().cursor = menu_root_log_out_index();

        // First Enter on the Log Out row pushes the prompt, cursor on No.
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        {
            let level = stack.current().expect("prompt pushed");
            assert!(
                matches!(level.kind, MenuKind::PowerPrompt { shut_down: false }),
                "Log Out must open its yes/no prompt, got {:?}",
                level.kind
            );
            assert_eq!(level.cursor, DROP_CONFIRM_DEFAULT, "prompt opens on No");
        }

        // Confirming while on No answers nothing and dismisses the prompt.
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        assert_eq!(stack.levels.len(), 1, "No dismisses back to Root");
        assert!(
            harness._cmd_rx.try_recv().is_err(),
            "No must not send a logout request"
        );

        // Re-open and answer Yes: the toggle goes out exactly once.
        stack.current_mut().unwrap().cursor = menu_root_log_out_index();
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        stack.current_mut().unwrap().cursor = 0; // Yes
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        assert_eq!(stack.levels.len(), 1, "answer dismisses the prompt");
        assert_eq!(
            harness._cmd_rx.try_recv().expect("Yes must send"),
            AgentCommand::ReqLogout {
                kind: ReqLogoutKind::LogoutToggle,
            }
        );
    }

    /// Direction keys never move the cursor onto a window's unselectable last item, and a
    /// window whose cursor rests on it ignores every key, Esc included
    /// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
    #[test]
    fn keys_skip_the_unselectable_item_and_a_cursor_resting_on_it_ignores_them() {
        use kuluu_render::input_mode::{LastItemLock, LastItemRow};
        const LAST: usize = 2;
        let mut harness = Harness::new();
        let mut world = marker_world();
        harness.dynamic.rows = (0..=LAST)
            .map(|_| kuluu_render::hud::menu::DynamicMenuRow {
                label: String::new(),
                action: kuluu_render::hud::menu::DynamicMenuAction::DisplayInfo,
                tint: None,
            })
            .collect();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        stack.current_mut().unwrap().last_item_lock = Some(LastItemLock {
            open_cursor: 0,
            row: LastItemRow::Row(LAST),
        });
        let mut press = |key: Key, code: KeyCode, stack: &mut MenuStack| {
            let markers = world.resource_mut::<MapMarkers>();
            harness.key(&key, code, stack, markers)
        };
        let cursor = |stack: &MenuStack| stack.current().unwrap().cursor;

        press(Key::ArrowDown, KeyCode::ArrowDown, &mut stack);
        assert_eq!(cursor(&stack), 1);
        press(Key::ArrowDown, KeyCode::ArrowDown, &mut stack);
        assert_eq!(cursor(&stack), 1, "Down onto the last item stays put");
        press(Key::ArrowUp, KeyCode::ArrowUp, &mut stack);
        press(Key::ArrowUp, KeyCode::ArrowUp, &mut stack);
        assert_eq!(cursor(&stack), 0, "Up's wrap onto the last item stays put");

        stack.current_mut().unwrap().cursor = LAST;
        press(Key::ArrowUp, KeyCode::ArrowUp, &mut stack);
        assert_eq!(cursor(&stack), LAST);
        assert!(press(Key::Escape, KeyCode::Escape, &mut stack).is_none());
        assert_eq!(
            stack.current().map(|level| level.kind),
            Some(MenuKind::RetailConfig),
            "Esc does not close it either"
        );
    }

    /// Shut Down and a Remodel style row raise the shared `comyn` confirm on No, as Log Out does
    /// (.agents/skills/retail-observe/references/2026-10-15-menu-docking-and-flags.md).
    #[test]
    fn shut_down_and_remodel_confirms_open_on_no() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.current_mut().unwrap().cursor = kuluu_render::hud::menu::root_entries()
            .iter()
            .position(|e| *e == kuluu_render::hud::menu::ROOT_SHUT_DOWN)
            .unwrap();
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        let level = stack.current().expect("prompt pushed");
        assert!(matches!(
            level.kind,
            MenuKind::PowerPrompt { shut_down: true }
        ));
        assert_eq!(level.cursor, ConfirmDefault::No.row());

        const STYLE: u16 = 615;
        harness.dynamic.rows = vec![kuluu_render::hud::menu::DynamicMenuRow {
            label: String::new(),
            action: kuluu_render::hud::menu::DynamicMenuAction::MogHouseRemodelSelect {
                style: STYLE,
            },
            tint: None,
        }];
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RoomList);
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        let level = stack.current().expect("prompt pushed");
        assert_eq!(level.kind, MenuKind::RoomListConfirm { style: STYLE });
        assert_eq!(level.cursor, ConfirmDefault::No.row());
    }

    fn menu_root_log_out_index() -> usize {
        use kuluu_render::hud::menu::ROOT_LOG_OUT;
        kuluu_render::hud::menu::root_entries()
            .iter()
            .position(|e| *e == ROOT_LOG_OUT)
            .unwrap()
    }

    #[test]
    fn region_info_select_installs_the_mode_in_place() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RegionInfo { mode: 0 });
        stack.take_absorb_open_minus();
        harness.dynamic.rows = kuluu_render::hud::menu::region_info_rows(0, None);

        // Handler item id 3 installs display mode 2 (vault main_menu.md §29).
        stack.current_mut().unwrap().cursor = 2;
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);

        assert_eq!(
            stack.levels.len(),
            2,
            "install must not push another window"
        );
        match stack.current().unwrap().kind {
            MenuKind::RegionInfo { mode } => assert_eq!(mode, 2),
            other => panic!("expected the Region Info window on top, got {other:?}"),
        }
    }

    /// User bug kuluu-tfmgd: one Enter with the page-1 cursor sitting on the
    /// Region Info row must open that window through the full key handler —
    /// the runtime path, not just label resolution. The kind is dynamic, so
    /// the registered refresh (hud/mod.rs) installs its live four-mode rows.
    #[test]
    fn region_info_root_row_opens_through_the_key_handler() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.current_mut().unwrap().cursor = kuluu_render::hud::menu::root_entries()
            .iter()
            .position(|e| *e == kuluu_render::hud::menu::ROOT_REGION_INFO)
            .expect("Region Info is a root row");
        {
            let markers = world.resource_mut::<MapMarkers>();
            harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        }
        assert_eq!(
            stack.levels.len(),
            2,
            "the Region Info window opens as its own level"
        );
        match stack.current().unwrap().kind {
            MenuKind::RegionInfo { mode } => assert_eq!(mode, 0),
            other => panic!("expected the Region Info window on top, got {other:?}"),
        }
        assert!(
            kuluu_render::hud::menu::is_dynamic(MenuKind::RegionInfo { mode: 0 }),
            "a dynamic kind gets its rows from the registered refresh system"
        );
    }

    #[test]
    fn mission_row_select_pushes_the_detail_window() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MissionList);
        stack.take_absorb_open_minus();
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        snap.missions.logs.push(kuluu_snapshot::MissionLogEntry {
            port: 0x00D0,
            data: [1, 0, 0, 0, 0, 0, 0, 0],
        });
        harness.scene_state.snapshot = snap;
        harness.dynamic.rows =
            kuluu_render::hud::menu::mission_list_rows(&harness.scene_state.snapshot);

        // The sole row is the Nations complete-log entry (vault main_menu.md section 30).
        stack.current_mut().unwrap().cursor = 0;
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);

        match stack.current().unwrap().kind {
            MenuKind::MissionDetail { port } => assert_eq!(port, 0x00D0),
            other => panic!("expected the miss00-stand-in detail on top, got {other:?}"),
        }
        assert_eq!(
            stack.levels.len(),
            3,
            "detail pushes over the list, like retail"
        );
    }

    #[test]
    fn quest_row_select_pushes_the_detail_window() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::QuestList);
        stack.take_absorb_open_minus();
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        // San d'Oria offer bit 0 only — the list's single row is quest (log 0, id 0).
        snap.missions.logs.push(kuluu_snapshot::MissionLogEntry {
            port: ffxi_proto::mission::QUEST_OFFER_PORTS[0],
            data: [1, 0, 0, 0, 0, 0, 0, 0],
        });
        harness.scene_state.snapshot = snap;
        harness.dynamic.rows =
            kuluu_render::hud::menu::quest_list_rows(&harness.scene_state.snapshot);

        stack.current_mut().unwrap().cursor = 0;
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);

        match stack.current().unwrap().kind {
            MenuKind::QuestDetail { log, id } => {
                assert_eq!(log, 0);
                assert_eq!(id, 0);
            }
            other => panic!("expected the quest01-stand-in detail on top, got {other:?}"),
        }
        assert_eq!(
            stack.levels.len(),
            3,
            "detail pushes over the list, like retail"
        );
    }

    fn menu_root_trade_index() -> usize {
        kuluu_render::hud::menu::root_entries()
            .iter()
            .position(|e| *e == "Trade")
            .unwrap()
    }

    /// Page-1 row 8 drills the trade window pair now — no longer a stub (vault
    /// main_menu.md §25).
    #[test]
    fn root_trade_row_resolves_to_the_window() {
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Trade", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::Trade)
        );
    }

    /// One Enter through the full key handler with an explicit target.
    fn drive_enter(
        harness: &mut Harness,
        stack: &mut MenuStack,
        world: &mut World,
        target_id: Option<u32>,
    ) {
        let markers = world.resource_mut::<MapMarkers>();
        handle_menu_key(
            &Key::Enter,
            KeyCode::Enter,
            &mut harness.bindings,
            stack,
            &mut harness.scene_state,
            &harness.cmd_tx,
            &mut harness.keybinds_state,
            &mut harness.graphics,
            &mut harness.hud_panels,
            &mut harness.net_status,
            &mut harness.audio_mute,
            &mut harness.vana_clock,
            &mut harness.vana_clock_visible,
            &mut harness.sort_options,
            &mut harness.item_menu_focus,
            &mut harness.item_bag,
            &mut harness.item_viewport,
            &mut harness.dynamic,
            target_id,
            kuluu_snapshot::Vec3::default(),
            &mut harness.map_state,
            markers,
            &harness.map_view,
            &harness.minimap_state,
            &harness.change_map_catalog,
            &harness.command_surface,
        );
    }

    fn trade_pc_entity(id: u32, name: &str) -> kuluu_snapshot::Entity {
        use kuluu_snapshot::{EntityKind, Vec3};
        kuluu_snapshot::Entity {
            id,
            act_index: id as u16,
            kind: EntityKind::Pc,
            name: Some(name.to_string()),
            pos: Vec3::default(),
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: kuluu_snapshot::status_type::NORMAL,
            char_flags: Default::default(),
            monstrosity: false,
            name_vis: None,
        }
    }

    /// Helper 0x13FE70's gate law (vault main_menu.md §25): without an eligible
    /// target the row opens nothing and sends nothing — the hint line stands in
    /// for retail's error-sound arm at menuwind 0x1406AD. A valid PC target
    /// puts c2s TRADE_REQ {UniqueNo, ActIndex} on the wire and opens our own
    /// windows right away; self-targets and a second concurrent trade are both
    /// refused exactly like the helper's null-field gates refuse them.
    #[test]
    fn root_trade_row_gates_on_eligible_target() {
        // No target at all: Root stays, nothing on the wire.
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.current_mut().unwrap().cursor = menu_root_trade_index();
        drive_enter(&mut harness, &mut stack, &mut world, None);
        assert_eq!(stack.levels.len(), 1, "no target: the window stays shut");
        assert!(harness._cmd_rx.try_recv().is_err());

        // A valid PC target opens the window and sends TRADE_REQ with both the
        // char id and the wire act index read off the entity.
        let mut harness = Harness::new();
        let mut world = marker_world();
        harness.scene_state.snapshot.entities = vec![trade_pc_entity(42, "Gessandral")];
        let mut stack = MenuStack::root();
        stack.current_mut().unwrap().cursor = menu_root_trade_index();
        drive_enter(&mut harness, &mut stack, &mut world, Some(42));
        assert_eq!(stack.current().unwrap().kind, MenuKind::Trade);
        assert_eq!(
            harness._cmd_rx.try_recv().expect("TRADE_REQ must go out"),
            AgentCommand::TradeRequest {
                char_id: 42,
                act_index: 42,
            }
        );

        // Self-target: the helper's self-entity comparison (call 0x81550) makes
        // this ineligible — no packet, no window.
        let mut harness = Harness::new();
        let mut world = marker_world();
        harness.scene_state.snapshot.self_char_id = Some(42);
        harness.scene_state.snapshot.entities = vec![trade_pc_entity(42, "Self")];
        let mut stack = MenuStack::root();
        stack.current_mut().unwrap().cursor = menu_root_trade_index();
        drive_enter(&mut harness, &mut stack, &mut world, Some(42));
        assert_eq!(stack.levels.len(), 1, "self is not a trade partner");
        assert!(harness._cmd_rx.try_recv().is_err());

        // Already trading: helper fields +0x74/+0x6C refuse the second open.
        let mut harness = Harness::new();
        let mut world = marker_world();
        harness.scene_state.snapshot.entities = vec![trade_pc_entity(42, "Gessandral")];
        harness.scene_state.snapshot.trade = Some(Box::new(kuluu_snapshot::TradeWindow {
            partner_id: 9,
            incoming_pending: false,
            active: true,
            my_slots: Vec::new(),
            partner_slots: Vec::new(),
            my_confirmed: false,
            partner_confirmed: false,
        }));
        let mut stack = MenuStack::root();
        stack.current_mut().unwrap().cursor = menu_root_trade_index();
        drive_enter(&mut harness, &mut stack, &mut world, Some(42));
        assert_eq!(stack.levels.len(), 1, "one transaction at a time (LSB law)");
        assert!(harness._cmd_rx.try_recv().is_err());
    }

    /// kuluu-u8p1: the Change Map list the key handler indexes and the one the
    /// panel draws must be the same list, and it must only offer floors the
    /// zone's DLL records describe -- POLUtils lists a third Windurst Waters map
    /// that the loader cannot resolve. Gated on a retail install (self-skips).
    #[test]
    fn change_map_confirm_selects_a_floor_the_dll_describes() {
        const WINDURST_WATERS: u16 = 238;
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let Ok(dll) = ffxi_dat::main_dll::MainDll::load(root.root()) else {
            return;
        };

        let mut harness = Harness::new();
        harness.change_map_catalog = ChangeMapCatalog::from_dll(&dll);
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Map);
        stack.take_absorb_open_minus();

        harness.scene_state.snapshot.zone_id = Some(WINDURST_WATERS);
        harness.map_state.mode = MapSubMode::ChangeMap;

        for _ in 0..2 {
            let markers = world.resource_mut::<MapMarkers>();
            harness.key(&Key::ArrowDown, KeyCode::ArrowDown, &mut stack, markers);
        }
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);

        // POLUtils counts three Windurst Waters maps and the DLL two, so under
        // the old roster the third row was this zone's phantom third floor.
        let viewed = harness.map_state.viewed.expect("a row was confirmed");
        assert_ne!(viewed, (WINDURST_WATERS, 2));
        assert_eq!(
            viewed.1, 0,
            "past the zone's floors the list moves on to other zones at map 0"
        );
        assert!(
            !dll.zone_maps(viewed.0).is_empty(),
            "and that zone has a DLL record to preview"
        );
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

    /// Drill law (bead kuluu-o9qt7.19.3): selecting a stored keytop row opens that
    /// slot's editor as its own level — retail chained the long-form mcr*edit
    /// windows off the palette/op-select handler (`FFXiMain.dll` retail-2026-09 RVA
    /// 0x1A68C0; vault main_menu.md §34/§41).
    #[test]
    fn macro_select_row_drills_the_line_editor() {
        use kuluu_render::hud::menu::{DynamicMenuAction, DynamicMenuRow};
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MacroSelect);
        harness.dynamic.rows = vec![DynamicMenuRow {
            tint: None,
            label: "B1/5: /ra".to_string(),
            action: DynamicMenuAction::MacroSlotRow { block: 0, index: 4 },
        }];
        stack.current_mut().unwrap().cursor = 0;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::MacroEditor { block: 0, index: 4 })
        );
    }

    /// Selecting a command-line row seeds the level's mcrledit buffer with that
    /// line (edit-and-commit law of FFXiMain.dll retail-2026-09 RVA 0x1A6180;
    /// vault main_menu.md §34/§41); selecting the add row seeds an empty append.
    #[test]
    fn editor_line_rows_seed_the_edit_buffer() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        harness.scene_state.snapshot.macros = vec![kuluu_snapshot::MacroSlot {
            block: 0,
            index: 3,
            text: "/ra".to_string(),
        }];
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MacroEditor { block: 0, index: 3 });
        harness.dynamic.rows =
            kuluu_render::hud::menu::macro_editor_rows(&harness.scene_state.snapshot, 0, 3);

        stack.current_mut().unwrap().cursor = 0; // the "/ra" line row
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::MacroEditor { block: 0, index: 3 }),
            "seeding must not leave the editor window"
        );
        let seeded = stack.current().unwrap().macro_line_edit.clone();
        assert_eq!(
            seeded,
            Some(kuluu_render::input_mode::MacroLineBuffer {
                block: 0,
                index: 3,
                line_no: Some(0),
                text: "/ra".to_string(),
            })
        );

        stack.current_mut().unwrap().macro_line_edit = None;
        stack.current_mut().unwrap().cursor = 2; // the add row (past line + use)
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().unwrap().macro_line_edit.clone(),
            Some(kuluu_render::input_mode::MacroLineBuffer {
                block: 0,
                index: 3,
                line_no: None,
                text: String::new(),
            })
        );
    }

    /// The clear row commits the store without the keytop — a blank keytop
    /// leaves no entry (vault main_menu.md §41 capacity law; bead .19.3).
    #[test]
    fn editor_clear_row_commits_the_store_without_the_slot() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        harness.scene_state.snapshot.macros = vec![
            kuluu_snapshot::MacroSlot {
                block: 0,
                index: 3,
                text: "/ra".to_string(),
            },
            kuluu_snapshot::MacroSlot {
                block: 1,
                index: 7,
                text: "/wait 1".to_string(),
            },
        ];
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MacroEditor { block: 0, index: 3 });
        harness.dynamic.rows =
            kuluu_render::hud::menu::macro_editor_rows(&harness.scene_state.snapshot, 0, 3);

        // Rows are ["/ra", use, add, clear] — the cursor goes to the clear row.
        stack.current_mut().unwrap().cursor = 3;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        let committed = match harness._cmd_rx.try_recv() {
            Ok(AgentCommand::CommitMacros { slots }) => slots,
            other => panic!("clear row must commit CommitMacros, got {other:?}"),
        };
        assert_eq!(
            committed,
            vec![kuluu_snapshot::MacroSlot {
                block: 1,
                index: 7,
                text: "/wait 1".to_string(),
            }],
            "the cleared keytop must be gone, the other untouched"
        );
    }

    /// Typing law (bead .19.3): printable keys append to the buffer, Backspace
    /// pops, Enter commits the merged set through CommitMacros and clears the
    /// buffer while the editor window stays open — retail's mcrledit blip-then-
    /// store tail (`FFXiMain.dll` retail-2026-09 handler 0x1A6180; vault
    /// main_menu.md §34/§41).
    #[test]
    fn editor_typing_commits_the_merged_slot() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        harness.scene_state.snapshot.macros = vec![kuluu_snapshot::MacroSlot {
            block: 0,
            index: 3,
            text: "/ra".to_string(),
        }];
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MacroEditor { block: 0, index: 3 });
        harness.dynamic.rows =
            kuluu_render::hud::menu::macro_editor_rows(&harness.scene_state.snapshot, 0, 3);

        // Seed through the add row so the commit is an append.
        stack.current_mut().unwrap().cursor = 2;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(stack.current().unwrap().macro_line_edit.is_some());

        for (key, code) in [
            (&Key::Character("/".into()), KeyCode::Slash),
            (&Key::Character("p".into()), KeyCode::KeyP),
            (&Key::Character("o".into()), KeyCode::KeyO),
            (&Key::Character("k".into()), KeyCode::KeyK),
            (&Key::Character("e".into()), KeyCode::KeyE),
        ] {
            harness.key(key, code, &mut stack, world.resource_mut::<MapMarkers>());
        }
        assert_eq!(
            stack
                .current()
                .unwrap()
                .macro_line_edit
                .as_ref()
                .unwrap()
                .text,
            "/poke"
        );

        // A typed space appends, Backspace pops it again (trailing-space edit).
        harness.key(
            &Key::Space,
            KeyCode::Space,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        harness.key(
            &Key::Backspace,
            KeyCode::Backspace,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );

        // Enter commits and clears.
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.current().unwrap().macro_line_edit, None);
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::MacroEditor { block: 0, index: 3 }),
            "retail's commit keeps the editor open"
        );
        let committed = match harness._cmd_rx.try_recv() {
            Ok(AgentCommand::CommitMacros { slots }) => slots,
            other => panic!("Enter must commit CommitMacros, got {other:?}"),
        };
        assert_eq!(
            committed,
            vec![kuluu_snapshot::MacroSlot {
                block: 0,
                index: 3,
                text: "/ra\n/poke".to_string(),
            }]
        );
    }

    /// While typing, navigation is dead (also the `.29` rule): arrows neither
    /// move the row cursor nor touch the buffer, and Esc discards the edit but
    /// keeps the editor window open (bead .19.3).
    #[test]
    fn editor_typing_owns_navigation_until_commit_or_escape() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MacroEditor { block: 0, index: 3 });
        let depth = stack.levels.len();
        stack.current_mut().unwrap().cursor = 1;
        stack.current_mut().unwrap().macro_line_edit =
            Some(kuluu_render::input_mode::MacroLineBuffer {
                block: 0,
                index: 3,
                line_no: Some(0),
                text: "/ra".to_string(),
            });

        for (key, code) in [
            (&Key::ArrowDown, KeyCode::ArrowDown),
            (&Key::ArrowUp, KeyCode::ArrowUp),
            (&Key::ArrowRight, KeyCode::ArrowRight),
        ] {
            harness.key(key, code, &mut stack, world.resource_mut::<MapMarkers>());
            assert_eq!(
                stack.current().unwrap().cursor,
                1,
                "arrows must not move rows while typing"
            );
            assert_eq!(
                stack
                    .current()
                    .unwrap()
                    .macro_line_edit
                    .as_ref()
                    .unwrap()
                    .text,
                "/ra",
                "arrows must not touch the buffer"
            );
        }

        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.current().unwrap().macro_line_edit, None);
        assert_eq!(stack.levels.len(), depth, "Esc discards the edit only");
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::MacroEditor { block: 0, index: 3 })
        );
    }

    /// The use row starts a run (bead kuluu-o9qt7.19.4): confirming it queues
    /// exactly the slot's command lines, in order — and nothing else: no store
    /// commit, no window change (retail runs one line at a time through the
    /// chat pipeline; vault main_menu.md §41/§56).
    #[test]
    fn editor_use_row_queues_the_slot_lines_in_order() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        harness.scene_state.snapshot.macros = vec![kuluu_snapshot::MacroSlot {
            block: 0,
            index: 3,
            text: "/ra\n/attend".to_string(),
        }];
        let mut stack = MenuStack::root();
        stack.push(MenuKind::MacroEditor { block: 0, index: 3 });
        harness.dynamic.rows =
            kuluu_render::hud::menu::macro_editor_rows(&harness.scene_state.snapshot, 0, 3);

        // Rows are ["/ra", "/attend", use, add, clear] — cursor on the use row.
        stack.current_mut().unwrap().cursor = 2;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(harness.scene_state.pending_macro_len(), 2);
        assert_eq!(
            (
                harness.scene_state.pop_next_macro_step(),
                harness.scene_state.pop_next_macro_step(),
                harness.scene_state.pop_next_macro_step()
            ),
            (Some("/ra".to_string()), Some("/attend".to_string()), None)
        );
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::MacroEditor { block: 0, index: 3 }),
            "running must not move the editor"
        );
        assert!(
            harness._cmd_rx.try_recv().is_err(),
            "a run is not a store write"
        );
    }

    /// Select law step 3 (statcom2 select handler FFXiMain.dll
    /// retail-2026-09 RVA 0x148D00 over detail table RVA 0x3716B8): a row
    /// select opens its detail companion beside the selector, and re-selecting
    /// the same row changes nothing.
    #[test]
    fn statcom_row_select_opens_detail_and_reselect_is_a_noop() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        // Display row 2 of the selector is Job Levels (statcom2 item id 2).
        stack.push(MenuKind::Status);
        stack.levels[1].cursor = 1;

        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(matches!(
            stack.levels.last().unwrap().kind,
            MenuKind::StatusDetail { entry: 1 }
        ));
        let depth = stack.levels.len();

        // The companion is display-only, so Enter lands on the selector again —
        // same row: retail changes nothing.
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.levels.len(), depth, "re-select is a no-op");
        assert!(matches!(
            stack.levels.last().unwrap().kind,
            MenuKind::StatusDetail { entry: 1 }
        ));
    }

    /// Bead kuluu-o9qt7.35 seam law: selecting a Currencies row queues exactly one
    /// header-only report request (`AgentCommand::RequestCurrencies`), and **every** select sends —
    /// the retail opener runs ahead of the window dedupe and caches nothing (FFXiMain.dll
    /// retail-2026-09 `.text 0xFD520`, mode discriminant from `CurrencyMode::as_wire`). Non-
    /// currency rows request nothing at all.
    #[test]
    fn currencies_row_select_queues_exactly_one_report_request_per_press() {
        use kuluu_render::hud::status_panel::{StatusEntryKind, STATUS_ENTRIES};
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        let cur_idx = STATUS_ENTRIES
            .iter()
            .position(|e| e.kind == StatusEntryKind::Currencies)
            .unwrap();
        stack.push(MenuKind::Status);
        stack.levels[1].cursor = cur_idx;

        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            harness._cmd_rx.try_recv(),
            Ok(AgentCommand::RequestCurrencies { mode: 0 }),
            "one select queues exactly one report-1 request"
        );
        assert!(matches!(
            stack.levels.last().unwrap().kind,
            MenuKind::StatusDetail { entry } if usize::from(entry) == cur_idx
        ));

        // The report window is NOT one of the six display-only details (it answers its own
        // commands), so it holds the keys while up — a second Enter belongs to that window,
        // not the selector, and must not double-send.
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(
            harness._cmd_rx.try_recv().is_err(),
            "the detail's own Enter requests nothing extra"
        );

        // Esc closes the report (one level), focus back on the selector with its cursor still
        // on Currencies; selecting again requests again — retail caches nothing, replies are
        // one-shot (FFXiMain.dll retail-2026-09 `.text 0x99620` clears after each reply).
        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.current().map(|l| l.kind), Some(MenuKind::Status));
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            harness._cmd_rx.try_recv(),
            Ok(AgentCommand::RequestCurrencies { mode: 0 }),
            "the re-select still requests"
        );

        // A plain row (Job Levels, no report family) queues nothing.
        let mut stack2 = MenuStack::root();
        stack2.push(MenuKind::Status);
        stack2.levels[1].cursor = 1;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack2,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(
            harness._cmd_rx.try_recv().is_err(),
            "a non-currency row requests nothing"
        );
    }

    /// Switch half of the select law (close-details helper FFXiMain.dll
    /// retail-2026-09 RVA 0x148AC0): switching rows closes the previous detail;
    /// never two details stack up.
    #[test]
    fn statcom_switch_replaces_the_previous_detail() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        // Rows 0 Profile and 1 Job Levels are the two adjacent rows retail never
        // gates; Master Levels (row 2) sits grey below level 99 (`statcom2` open code
        // FFXiMain.dll retail-2026-09 `.text 0x148B50`) and would answer nothing here.
        stack.levels[1].cursor = 0;

        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        // Move the SELECTOR (pass-through keys) and select another row.
        harness.key(
            &Key::ArrowDown,
            KeyCode::ArrowDown,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.levels.len(), 3, "one detail window only");
        assert!(matches!(
            stack.levels.last().unwrap().kind,
            MenuKind::StatusDetail { entry: 1 }
        ));
    }

    /// Item id 9 of the selector's dispatch table: Play Time runs `/playtime`
    /// and opens no window (FFXiMain.dll retail-2026-09 RVA 0x148D00; vault
    /// main_menu.md §59).
    #[test]
    fn statcom_play_time_row_runs_the_command_without_a_window() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        // Play Time is display row 10 (statcom2 item id 9).
        stack.levels[1].cursor = 9;

        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.levels.len(), 2, "no detail window for Play Time");
        let toast = harness
            .scene_state
            .local_toasts
            .last()
            .expect("playtime line");
        assert!(toast.text.contains("Play time:"), "{:?}", toast.text);
    }

    /// Retail opens Combat/Magic/Craft disabled and only content in s2c 0x062 lifts
    /// them (`.text FFXiMain.dll retail-2026-09 0x148B50`), so selecting the row before
    /// that block arrives must answer nothing at all — no empty window, since the
    /// detail handler is never reached.
    #[test]
    fn statcom_skill_rows_answer_only_once_the_block_arrives() {
        use kuluu_render::hud::status_panel::StatusEntryKind;
        const COMBAT_ROW: usize = 3;

        // No block yet: the row stays grey and its select is inert.
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        stack.levels[1].cursor = COMBAT_ROW;
        assert_eq!(
            kuluu_render::hud::status_panel::STATUS_ENTRIES[COMBAT_ROW].kind,
            StatusEntryKind::CombatSkill
        );

        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.levels.len(), 2, "a gated row opens no detail window");

        // With one combat word in the block, the same key press opens `btlskill`.
        let mut block = kuluu_snapshot::CharSkills::zeroed();
        const HAND_TO_HAND_LEVEL: u16 = 301;
        block.skills[1] = HAND_TO_HAND_LEVEL;
        harness.scene_state.snapshot.char_skills = Some(block);

        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.levels.len(), 3, "the row answers once content exists");
        assert!(matches!(
            stack.levels.last().unwrap().kind,
            MenuKind::StatusDetail { entry } if entry as usize == COMBAT_ROW
        ));
    }

    /// The focus law (vault main_menu.md §59, statcom2 registry flag 0x1 companions):
    /// while a display-only detail tops the stack its keys stay on the selector
    /// — Down moves the selector cursor, never the companion's.
    #[test]
    fn display_only_detail_keeps_the_selector_keys() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        stack.levels[1].cursor = 0;

        // Open Profile (display-only), then navigate: the selector moves.
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(matches!(
            stack.levels.last().unwrap().kind,
            MenuKind::StatusDetail { entry: 0 }
        ));
        harness.key(
            &Key::ArrowDown,
            KeyCode::ArrowDown,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.levels.len(), 3, "still selector + one companion");
        assert_eq!(
            stack.levels[1].cursor, 1,
            "the selector underneath keeps navigating"
        );
    }

    /// Bead kuluu-o9qt7.29 regression: a Left/Right stepper row must never
    /// swallow vertical travel while it holds the cursor (the camera_leash bug).
    #[test]
    fn stepper_row_never_swallows_up_down_or_esc() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Debug);
        let rows = kuluu_render::hud::menu::entries_for(MenuKind::Debug, false);
        let idx = rows
            .iter()
            .position(|l| *l == kuluu_render::hud::menu::RETAIL_CAMERA_LEASH)
            .expect("camera_leash row exists");
        stack.levels[1].cursor = idx;
        // Park the value one step below its default-ish middle so a Right press
        // must raise it; `moved` is that same step's midpoint check.
        use kuluu_render::graphics_settings::CAMERA_LEASH_STEP_YALMS as LEASH_STEP;
        let park = 4.0 * LEASH_STEP;
        harness.graphics.camera_leash_yalms = park;

        harness.key(
            &Key::ArrowRight,
            KeyCode::ArrowRight,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(
            harness.graphics.camera_leash_yalms > park,
            "stepper consumed its own axis"
        );

        // After the stepper press, Up still moves the cursor off the row.
        harness.key(
            &Key::ArrowUp,
            KeyCode::ArrowUp,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().unwrap().cursor,
            idx - 1,
            "vertical travel survives a stepper row (bead .29)"
        );

        // And Esc still pops the window while parked on the stepper.
        stack.current_mut().unwrap().cursor = idx;
        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(matches!(stack.current().unwrap().kind, MenuKind::Root));
    }

    /// Esc closes the detail before the selector (statcom2 flag 0x2 law: the
    /// handler's close-details step first, then itself — cascade outwards).
    #[test]
    fn esc_closes_the_detail_then_the_selector() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);

        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.levels.len(), 3);
        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(matches!(stack.current().unwrap().kind, MenuKind::Status,));
        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(matches!(stack.current().unwrap().kind, MenuKind::Root));
    }
    /// The runtime pin law of `conf2win` (bead kuluu-o9qt7.20.1 part B, handler
    /// FFXiMain.dll retail-2026-09 RVA 0x197470 key tables .text 0x197874/
    /// 0x19788C): Enter pins without writing the store; Left/Right step ±1
    /// writing each step, dying at the clamps (0 and CONFIG_SLIDER_MAX); Esc
    /// saves-and-leaves — it unpins but never pops the window. Any other top window drops a
    /// stale pin. Up/Down are not part of this law: they keep moving rows
    /// (`slider_pin_never_silences_vertical_travel`, bead kuluu-8l5h4).
    #[test]
    fn conf2win_slider_pin_steps_clamps_and_swallows() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows};
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf2win".to_string(),
            (0..11u32).map(|i| (700 + i, format!("srow{i}"))).collect(),
        );
        harness.dynamic.rows = config_window_rows(
            "conf2win",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigPage(0));
        // Row 4 selects store 0xA — boots at its max, so Right must die first.
        stack.current_mut().unwrap().cursor = 3;
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit,
            Some((0xAu32, 100u8)),
            "the entry arm pins at the stored-or-boot value without writing"
        );
        assert!(
            !harness.dynamic.config_option_values.contains_key(&0xAu32),
            "pinning alone writes nothing (entry arms stash only)"
        );

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowRight, KeyCode::ArrowRight, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit,
            Some((0xA, 100)),
            "at the ceiling Right is swallowed"
        );
        for expected in [99u8, 98] {
            let markers = world.resource_mut::<MapMarkers>();
            harness.key(&Key::ArrowLeft, KeyCode::ArrowLeft, &mut stack, markers);
            assert_eq!(harness.dynamic.option_edit, Some((0xA, expected)));
            assert_eq!(
                harness.dynamic.config_option_values.get(&0xAu32),
                Some(&i32::from(expected)),
                "each step writes the store immediately"
            );
        }

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Escape, KeyCode::Escape, &mut stack, markers);
        assert!(
            matches!(stack.current().unwrap().kind, MenuKind::RetailConfigPage(0)),
            "Esc is the save-and-leave arm — it must not pop the window"
        );
        assert_eq!(harness.dynamic.option_edit, None);
        assert_eq!(harness.dynamic.config_option_values.get(&0xAu32), Some(&98));

        // Re-pin picks up the last written value, and a pin cannot outlive its
        // window: closing back to Root drops it.
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        assert_eq!(harness.dynamic.option_edit, Some((0xAu32, 98u8)));
        stack.pop();
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowUp, KeyCode::ArrowUp, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit, None,
            "the pin belongs to conf2win only"
        );
    }

    /// A pinned slider owns Left/Right and nothing else. Retail routes every direction through
    /// its own neighbour byte in the window input switch (FFXiMain.dll retail-2026-09
    /// RVA 0x118260), so pressing Down while a value is being stepped must drop the pin and land
    /// on the next row — never freeze the cursor (bead kuluu-8l5h4; vault main_menu.md §5 nav law).
    #[test]
    fn slider_pin_never_silences_vertical_travel() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows};
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf2win".to_string(),
            (0..11u32).map(|i| (700 + i, format!("srow{i}"))).collect(),
        );
        harness.dynamic.rows = config_window_rows(
            "conf2win",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigPage(0));
        let row_count = harness.dynamic.rows.len();
        stack.current_mut().unwrap().cursor = 3;

        // Pin the store, then step it once so there is something on the row.
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        assert!(harness.dynamic.option_edit.is_some(), "row 3 pins");
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowLeft, KeyCode::ArrowLeft, &mut stack, markers);
        let stepped = harness.dynamic.config_option_values.get(&0xAu32).copied();

        // Down: the pin goes and the cursor moves — the same step an unpinned row takes.
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowDown, KeyCode::ArrowDown, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit, None,
            "stepping off drops the pin"
        );
        assert_eq!(stack.current().unwrap().cursor, 4, "Down moved a row");

        // Up comes straight back onto the slider row with the value it had left behind.
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowUp, KeyCode::ArrowUp, &mut stack, markers);
        assert_eq!(stack.current().unwrap().cursor, 3);
        assert_eq!(
            harness.dynamic.config_option_values.get(&0xAu32).copied(),
            stepped,
            "leaving the row keeps what was stepped"
        );
        assert_eq!(row_count, 11, "the fixture is the eleven-row conf2win");
    }

    /// `conf3win`'s slider row (row 11) answers the same pin law but on its own
    /// store 0x3C with a floor: entry arms write nothing (.text FFXiMain.dll
    /// retail-2026-09 RVA 0x19AAFB), steps land in the store between 50 and 25
    /// (`cmp eax,0x32/jge` at .text 0x19ABF7 and `cmp eax,0x19/jle` at .text
    /// 0x19ABDB; vault main_menu.md §59 addendum 6).
    #[test]
    fn conf3win_slider_steps_between_its_floor_and_ceiling() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows};
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf3win".to_string(),
            (0..15u32).map(|i| (600 + i, format!("c3row{i}"))).collect(),
        );
        harness.dynamic.rows = config_window_rows(
            "conf3win",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigPage(4));
        stack.current_mut().unwrap().cursor = 10;
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit,
            Some((0x3Cu32, 50u8)),
            "row 11 pins store 0x3C at its boot value without writing"
        );

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowRight, KeyCode::ArrowRight, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit,
            Some((0x3C, 50)),
            "its ceiling swallows Right"
        );

        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowLeft, KeyCode::ArrowLeft, &mut stack, markers);
        assert_eq!(harness.dynamic.option_edit, Some((0x3C, 49)));
        assert_eq!(
            harness.dynamic.config_option_values.get(&0x3Cu32),
            Some(&49),
            "the step writes store 0x3C"
        );

        // At the floor another Left does nothing at all.
        harness.dynamic.option_edit = Some((0x3C, 25));
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowLeft, KeyCode::ArrowLeft, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit,
            Some((0x3C, 25)),
            "the decrement arm only steps above 0x19"
        );
        assert_eq!(
            harness.dynamic.config_option_values.get(&0x3Cu32),
            Some(&49)
        );
    }

    /// The individual log-window editors (`conf5w1`/`conf5w2`, drilled by `conf5m`
    /// ids 2/3) run the shared stepper law plus their per-store extras (vault
    /// main_menu.md §59 addendum 46): column width steps by ten and clamps after the
    /// arithmetic (`add eax,-0xa` fix-up at FFXiMain.dll retail-2026-09 .text
    /// 0x199A4B/0x199A55), the interval lives between store values 3 and 30 (guards on
    /// its raw field, ±3 shifted), and a line-count pair stays ordered across stores —
    /// lowering max pulls min down (.text 0x199AEF..0x199B15) and raising min lifts max
    /// back up (.text 0x199BBF..0x199BE8).
    #[test]
    fn log_window_steppers_step_by_ten_and_keep_line_counts_ordered() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows};
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf5w1".to_string(),
            (0..6u32).map(|i| (800 + i, format!("logrow{i}"))).collect(),
        );
        harness.dynamic.rows = config_window_rows(
            "conf5w1",
            &table,
            &std::collections::BTreeMap::new(),
            None,
            &std::collections::BTreeMap::new(),
        );
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigSub(1));

        // Column width (row 5, cursor 4) pins at its boot value and steps by ten.
        stack.current_mut().unwrap().cursor = 4;
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit,
            Some((0x9Cu32, 100u8)),
            "the width row pins at boot 100 (constructor field +0x28)"
        );
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowRight, KeyCode::ArrowRight, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit,
            Some((0x9C, 100)),
            "ceiling swallows Right"
        );
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowLeft, KeyCode::ArrowLeft, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit,
            Some((0x9C, 90)),
            "width steps by ten"
        );

        // Five down the ladder another Left lands on the clamp point, not below it.
        harness.dynamic.option_edit = Some((0x9C, 5));
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowLeft, KeyCode::ArrowLeft, &mut stack, markers);
        assert_eq!(harness.dynamic.option_edit, Some((0x9C, 0)));
        assert_eq!(harness.dynamic.config_option_values.get(&0x9Cu32), Some(&0));

        // Interval (row 6, cursor 5): boot 8, store floors at 3 and ceilings at 30.
        harness.dynamic.option_edit = Some((0x11u32, 4u8));
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowLeft, KeyCode::ArrowLeft, &mut stack, markers);
        assert_eq!(harness.dynamic.option_edit, Some((0x11, 3)));
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowLeft, KeyCode::ArrowLeft, &mut stack, markers);
        assert_eq!(
            harness.dynamic.option_edit,
            Some((0x11, 3)),
            "raw floor is store value 3"
        );
        harness.dynamic.option_edit = Some((0x11, 29));
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowRight, KeyCode::ArrowRight, &mut stack, markers);
        assert_eq!(harness.dynamic.option_edit, Some((0x11, 30)));

        // Max-lines (row 3, cursor 2) with the pair inverted {max 6, min 7}: stepping
        // down clamps min to the new maximum. Drop the interval pin first so Enter
        // arms this row instead of saving-and-leaving the old pin.
        harness.dynamic.option_edit = None;
        harness.dynamic.config_option_values.insert(0xFu32, 6);
        harness.dynamic.config_option_values.insert(0x10u32, 7);
        stack.current_mut().unwrap().cursor = 2;
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::Enter, KeyCode::Enter, &mut stack, markers);
        assert_eq!(harness.dynamic.option_edit, Some((0xFu32, 6u8)));
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowLeft, KeyCode::ArrowLeft, &mut stack, markers);
        assert_eq!(harness.dynamic.option_edit, Some((0xF, 5)));
        assert_eq!(
            harness.dynamic.config_option_values.get(&0x10u32),
            Some(&5),
            "min-lines is dragged down to the new maximum"
        );

        // Min-lines (row 4) stepping above its max drags the maximum back up.
        harness.dynamic.option_edit = Some((0x10u32, 6u8));
        let markers = world.resource_mut::<MapMarkers>();
        harness.key(&Key::ArrowRight, KeyCode::ArrowRight, &mut stack, markers);
        assert_eq!(harness.dynamic.option_edit, Some((0x10, 7)));
        assert_eq!(
            harness.dynamic.config_option_values.get(&0xFu32),
            Some(&7),
            "max-lines is raised to the new minimum"
        );
    }

    #[test]
    fn emote_list_opens_the_abisortw_pair_below_it() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Communication);
        stack.current_mut().unwrap().cursor = 1; // COMM_EMOTE_LIST
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        let kinds: Vec<MenuKind> = stack.levels.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                MenuKind::Root,
                MenuKind::Communication,
                MenuKind::AbilitySort { mode: 1 },
                MenuKind::EmoteList,
            ],
            "companion opens first, the captioned list takes focus on top"
        );
    }

    /// Esc law for the pair (bead kuluu-o9qt7.34): with a swap pending, Esc answers the
    /// emote window's cancel arm — state resets, nothing closes (.text FFXiMain.dll
    /// retail-2026-09 `0x225E34`); once settled, closing the list cascades over the
    /// companion (`.text 0x225A88`) so one Esc brings both windows down.
    #[test]
    fn emote_list_esc_cancels_pending_swap_then_cascades_the_companion() {
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::AbilitySort { mode: 1 });
        stack.push(MenuKind::EmoteList);

        harness.dynamic.emote_sort_state = kuluu_render::hud::menu::EmoteSortState::AwaitFirst;
        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            harness.dynamic.emote_sort_state,
            kuluu_render::hud::menu::EmoteSortState::Off,
            "the cancel arm resets the state word"
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::EmoteList,
            "…and keeps both windows up"
        );

        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::Root,
            "a settled close takes the companion with it"
        );
    }

    /// The swap FSM end to end (bead kuluu-o9qt7.34): chain row → AwaitFirst; first pick
    /// → AwaitSecond(id); second pick commits the full blob through CommitEmoteOrder, and
    /// retail's tail runs in place — refill with state reset (.text FFXiMain.dll
    /// retail-2026-09 0x2256F0/0x225960) and row B selected.
    #[test]
    fn emote_swap_fsm_commits_the_blob_and_refills() {
        use kuluu_render::hud::menu::{
            append_abisortw_chain_row, emote_rows, DynamicMenuAction, EmoteSortState,
        };
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::AbilitySort { mode: 1 });
        stack.push(MenuKind::EmoteList);

        let mut rows = emote_rows(&harness.scene_state.snapshot);
        assert!(
            rows.len() >= 2,
            "the canned table has at least two visible rows"
        );
        append_abisortw_chain_row(&mut rows, Some("Rearrange"));
        let chain_cursor = rows.len() - 1;
        harness.dynamic.rows = rows;

        // Chain row select arms the FSM — list stays up, no wire.
        stack.current_mut().unwrap().cursor = chain_cursor;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(harness.dynamic.emote_sort_state, EmoteSortState::AwaitFirst);
        assert_eq!(stack.current().unwrap().kind, MenuKind::EmoteList);

        // First pick (row 0) lands in the pending word; still no wire.
        let first_id = match harness.dynamic.rows[0].action {
            DynamicMenuAction::Emote { emote_id } => emote_id,
            other => panic!("row 0 must be an emote, got {other:?}"),
        };
        stack.current_mut().unwrap().cursor = 0;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            harness.dynamic.emote_sort_state,
            EmoteSortState::AwaitSecond(first_id)
        );
        assert!(
            harness._cmd_rx.try_recv().is_err(),
            "pending picks send nothing"
        );

        // Second pick (row 1) commits; the guard pair would be covered by the pure
        // law tests — here the happy path only.
        let second_id = match harness.dynamic.rows[1].action {
            DynamicMenuAction::Emote { emote_id } => emote_id,
            other => panic!("row 1 must be an emote, got {other:?}"),
        };
        stack.current_mut().unwrap().cursor = 1;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        let committed = match harness._cmd_rx.try_recv() {
            Ok(AgentCommand::CommitEmoteOrder { ids }) => ids,
            other => panic!("second pick must commit CommitEmoteOrder, got {other:?}"),
        };
        // Positions come from the untouched fill order (empty store = default table):
        // after the swap those two slots hold each other's pick, nothing else moves.
        let defaults: Vec<u8> = ffxi_vocab::emote_names::EMOTES
            .iter()
            .map(|&(id, _)| id)
            .collect();
        let a = defaults
            .iter()
            .position(|&id| id == first_id)
            .expect("first pick is a real emote row");
        let b = defaults
            .iter()
            .position(|&id| id == second_id)
            .expect("second pick is a real emote row");
        assert_eq!(
            committed.len(),
            defaults.len(),
            "the whole blob round-trips"
        );
        assert_eq!(committed[a], second_id);
        assert_eq!(committed[b], first_id);

        // Refill law: rows rebuilt from the new blob, state Off, row B selected.
        assert_eq!(harness.dynamic.emote_sort_state, EmoteSortState::Off);
        assert!(
            matches!(
                harness.dynamic.rows.first().unwrap().action,
                DynamicMenuAction::Emote { emote_id } if emote_id == second_id
            ),
            "the stored pair leads the refilled list"
        );
        assert_eq!(
            stack.current().unwrap().cursor,
            0,
            "row B — the second pick — is selected"
        );
    }

    fn status_row_of(kind: kuluu_render::hud::status_panel::StatusEntryKind) -> usize {
        kuluu_render::hud::status_panel::STATUS_ENTRIES
            .iter()
            .position(|e| e.kind == kind)
            .expect("status row")
    }

    fn drain_commands(harness: &mut Harness) -> Vec<AgentCommand> {
        std::iter::from_fn(|| harness._cmd_rx.try_recv().ok()).collect()
    }

    /// Status row id 11 opens `ut_menu` over the selector, asking for every ranking set whose
    /// table completed (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md);
    /// the row is never disabled, and with a display-only detail up the detail closes first.
    #[test]
    fn unity_row_opens_ut_menu_and_requests_completed_sets() {
        use kuluu_render::hud::status_panel::StatusEntryKind;
        let mut harness = Harness::new();
        let mut world = marker_world();
        harness.scene_state.snapshot.unity.rankings[1].complete = true;
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        stack.levels[1].cursor = 0;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(matches!(
            stack.current().unwrap().kind,
            MenuKind::StatusDetail { entry: 0 }
        ));
        stack.levels[1].cursor = status_row_of(StatusEntryKind::Unity);
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.levels.iter().map(|l| l.kind).collect::<Vec<_>>(),
            vec![MenuKind::Root, MenuKind::Status, MenuKind::UnityMenu]
        );
        assert_eq!(
            drain_commands(&mut harness),
            vec![AgentCommand::RequestUnityRanking { result_set: 1 }]
        );
    }

    /// Status row id 13 opens `fp_cat` once its gate holds, asking for the alter ego point data
    /// while none has arrived; Esc closes it back to the selector.
    #[test]
    fn alter_ego_row_opens_fp_cat_and_requests_points_without_data() {
        use kuluu_render::hud::status_panel::{
            StatusEntryKind, ALTER_EGO_POINTS_KEY_ITEM, MASTER_LEVEL_JOB_CAP,
        };
        let mut harness = Harness::new();
        let mut world = marker_world();
        let snap = &mut harness.scene_state.snapshot;
        snap.status_data_ready = true;
        snap.self_char_id = Some(7);
        snap.party.push(kuluu_snapshot::PartyMember {
            id: 7,
            act_index: 1,
            name: None,
            hp: 1,
            mp: 0,
            tp: 0,
            hp_pct: 100,
            mp_pct: 0,
            zone_no: 1,
            main_job: 1,
            main_job_lv: MASTER_LEVEL_JOB_CAP,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: true,
            party_no: 0,
        });
        snap.key_items.push(ALTER_EGO_POINTS_KEY_ITEM);
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        stack.levels[1].cursor = status_row_of(StatusEntryKind::FacetCategories);
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.current().unwrap().kind, MenuKind::AlterEgoCategories);
        assert_eq!(
            drain_commands(&mut harness),
            vec![AgentCommand::RequestAlterEgoPoints]
        );
        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.current().unwrap().kind, MenuKind::Status);
    }

    /// The `merityn` Yes on an affordable upgrade in the Mog House sends c2s 0x0C1; selecting the
    /// row outside the Mog House prints the table 7 refusal and queues the buzzer instead.
    #[test]
    fn upgrade_confirm_sends_and_refusal_prints_and_buzzes() {
        use kuluu_render::hud::menu::{DynamicMenuAction, DynamicMenuRow, UiSound};
        const MP: u16 = 9;
        const CAP: u8 = 50;
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut points = kuluu_snapshot::AlterEgoPoints {
            points: 10,
            upgrades: [0; kuluu_snapshot::ALTER_EGO_UPGRADE_SLOTS],
            costs: [0; kuluu_snapshot::ALTER_EGO_UPGRADE_SLOTS],
        };
        points.costs[usize::from(MP)] = 2;
        harness.scene_state.snapshot.alter_ego_points = Some(points);
        harness.scene_state.snapshot.login_state =
            ffxi_proto::decode::ServerLoginMyroom::LOGIN_STATE_GAME;
        harness.dynamic.unity_fp.not_allowed_message = Some("refused".into());
        harness.dynamic.rows = vec![DynamicMenuRow {
            label: "MP".into(),
            action: DynamicMenuAction::AlterEgoUpgrade {
                upgrade: MP,
                cap: CAP,
            },
            tint: None,
        }];
        let mut stack = MenuStack::root();
        stack.push(MenuKind::AlterEgoCategories);
        stack.push(MenuKind::AlterEgoUpgrades { category: 1 });
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            harness
                .scene_state
                .local_toasts
                .last()
                .map(|l| l.text.as_str()),
            Some("refused")
        );
        assert_eq!(harness.dynamic.ui_sounds, vec![UiSound::Buzzer]);
        assert_eq!(stack.levels.len(), 3, "nothing opens");

        harness.scene_state.snapshot.login_state =
            ffxi_proto::decode::ServerLoginMyroom::LOGIN_STATE_MYROOM;
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().map(|l| (l.kind, l.cursor)),
            Some((MenuKind::AlterEgoConfirm { upgrade: MP }, 1)),
            "the prompt opens on No"
        );
        harness.dynamic.rows = vec![
            DynamicMenuRow {
                label: String::new(),
                action: DynamicMenuAction::AlterEgoConfirmAnswer { yes: true },
                tint: None,
            },
            DynamicMenuRow {
                label: String::new(),
                action: DynamicMenuAction::AlterEgoConfirmAnswer { yes: false },
                tint: None,
            },
        ];
        harness.key(
            &Key::ArrowUp,
            KeyCode::ArrowUp,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            drain_commands(&mut harness),
            vec![AgentCommand::UpgradeAlterEgo { upgrade_id: MP }]
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::AlterEgoUpgrades { category: 1 }
        );
    }

    fn merit_item(item: u8, enabled: bool) -> kuluu_render::hud::menu::DynamicMenuRow {
        kuluu_render::hud::menu::DynamicMenuRow {
            label: String::new(),
            action: kuluu_render::hud::menu::DynamicMenuAction::Merit(
                kuluu_render::hud::merit_windows::MeritAction::Item { item, enabled },
            ),
            tint: None,
        }
    }

    /// Status row id 7 opens `merit1` once its record gate holds; row id 8 opens `merit2ca` and
    /// asks for the job point data while none has arrived
    /// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
    #[test]
    fn status_rows_7_and_8_open_the_merit_and_job_point_windows() {
        use kuluu_render::hud::merit_windows::MeritWindow;
        use kuluu_render::hud::status_panel::{
            StatusEntryKind, JOB_POINTS_ACCESS_BIT, MASTER_LEVEL_JOB_CAP,
        };
        let mut harness = Harness::new();
        let mut world = marker_world();
        let snap = &mut harness.scene_state.snapshot;
        snap.merits = Some(kuluu_snapshot::MisdataMerits {
            limit_points: 0,
            merit_points: 0,
            blu_bonus: 0,
            can_use_merit_mode: true,
            xp_capped_or_merit_mode: false,
            merit_mode_enabled: false,
            max_merit_points: 0,
        });
        snap.job_points = Some(kuluu_snapshot::JobPointsRecord {
            access: JOB_POINTS_ACCESS_BIT,
            ..Default::default()
        });
        snap.stats = Some(kuluu_snapshot::CharStats {
            mjob_lv: MASTER_LEVEL_JOB_CAP,
            ..Default::default()
        });
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        stack.levels[1].cursor = status_row_of(StatusEntryKind::MeritPoints);
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::Merit(MeritWindow::Menu)
        );
        assert!(drain_commands(&mut harness).is_empty());
        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(stack.current().unwrap().kind, MenuKind::Status);

        stack.levels[1].cursor = status_row_of(StatusEntryKind::JobPoints);
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::Merit(MeritWindow::Jobs)
        );
        assert_eq!(
            drain_commands(&mut harness),
            vec![AgentCommand::RequestJobPoints]
        );
    }

    /// `merit2`'s enabled item sends c2s 0x0BE ChangeMode; `merityn` takes Esc as a buzzer only,
    /// and its Yes sends EditMode.
    #[test]
    fn merit_mode_and_confirm_answer_through_the_keys() {
        use kuluu_render::hud::menu::UiSound;
        use kuluu_render::hud::merit_windows::{
            MeritWindow, MERIT2_ITEM_EXP, MERIT2_ITEM_LIMIT_POINTS,
        };
        use kuluu_render::hud::unity_fp::{CONFIRM_ITEM_NO, CONFIRM_ITEM_YES};
        const MERIT_INDEX: u16 = 0x20;
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Status);
        stack.push(MenuKind::Merit(MeritWindow::Menu));
        stack.push(MenuKind::Merit(MeritWindow::Mode));
        harness.dynamic.rows = vec![
            merit_item(MERIT2_ITEM_EXP, false),
            merit_item(MERIT2_ITEM_LIMIT_POINTS, true),
        ];
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert!(
            drain_commands(&mut harness).is_empty(),
            "item 1 is disabled"
        );
        harness.key(
            &Key::ArrowDown,
            KeyCode::ArrowDown,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            drain_commands(&mut harness),
            vec![AgentCommand::SetMeritMode { limit_points: true }]
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::Merit(MeritWindow::Mode),
            "merit2 stays open"
        );

        let confirm = MenuKind::Merit(MeritWindow::Confirm {
            index: MERIT_INDEX,
            raise: true,
        });
        stack.push(confirm);
        harness.dynamic.rows = vec![
            merit_item(CONFIRM_ITEM_YES, true),
            merit_item(CONFIRM_ITEM_NO, true),
        ];
        harness.key(
            &Key::Escape,
            KeyCode::Escape,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().unwrap().kind,
            confirm,
            "Esc does not answer"
        );
        assert_eq!(harness.dynamic.ui_sounds, vec![UiSound::Buzzer]);
        harness.key(
            &Key::Enter,
            KeyCode::Enter,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            drain_commands(&mut harness),
            vec![AgentCommand::EditMerit {
                index: MERIT_INDEX,
                raise: true
            }]
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::Merit(MeritWindow::Mode)
        );
    }

    /// Left and Right flip `jbpcat` between its Job Points and Gifts pages with the page toggle
    /// sound instead of paging the list.
    #[test]
    fn left_right_toggle_the_job_point_page() {
        use kuluu_render::hud::menu::UiSound;
        use kuluu_render::hud::merit_windows::MeritWindow;
        const WAR: u8 = 1;
        let mut harness = Harness::new();
        let mut world = marker_world();
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Merit(MeritWindow::Jobs));
        stack.push(MenuKind::Merit(MeritWindow::JobCategories {
            job: WAR,
            gifts: false,
        }));
        harness.key(
            &Key::ArrowRight,
            KeyCode::ArrowRight,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::Merit(MeritWindow::JobCategories {
                job: WAR,
                gifts: true
            })
        );
        harness.key(
            &Key::ArrowLeft,
            KeyCode::ArrowLeft,
            &mut stack,
            world.resource_mut::<MapMarkers>(),
        );
        assert_eq!(
            stack.current().unwrap().kind,
            MenuKind::Merit(MeritWindow::JobCategories {
                job: WAR,
                gifts: false
            })
        );
        assert_eq!(
            harness.dynamic.ui_sounds,
            vec![UiSound::PageToggle, UiSound::PageToggle]
        );
    }
}

#[cfg(test)]
mod menu_dispatch_tests {
    use super::*;

    #[test]
    fn log_out_row_opens_the_yes_no_prompt() {
        use kuluu_render::hud::menu::ROOT_LOG_OUT;
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, ROOT_LOG_OUT, false, false),
            MenuDispatch::OpenSubmenu(MenuKind::PowerPrompt { shut_down: false })
        );
    }

    #[test]
    fn shut_down_row_opens_the_yes_no_prompt() {
        use kuluu_render::hud::menu::ROOT_SHUT_DOWN;
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, ROOT_SHUT_DOWN, false, false),
            MenuDispatch::OpenSubmenu(MenuKind::PowerPrompt { shut_down: true })
        );
    }

    /// The Search row drills into the searchma/scresult surface unconditionally,
    /// like retail's menuwind arm 0x140626 opens `searchma` with no gate (vault
    /// main_menu.md §26).
    #[test]
    fn search_row_opens_the_search_window() {
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Search", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::Search)
        );

        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Linkshell", false, false),
            MenuDispatch::OpenSubmenu(kuluu_render::MenuKind::Linkshell),
            "page-1 row 10 drills to the slot window (retail always opens menu    link5)"
        );
    }

    /// The Synthesis row drills the cmbmenu launcher ungated, exactly like the
    /// menuwind arm FFXiMain.dll retail-2026-09 RVA 0x1405D2 name-opens `menu
    /// cmbmenu` with no gate (vault main_menu.md §28), and the launcher's two rows
    /// go where select ids 1/2 of handler 0x1E29A0 take them (§22).
    #[test]
    fn synthesis_row_opens_the_cmbmenu_launcher() {
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Synthesis", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::SynthLauncher)
        );
        use kuluu_render::hud::menu::{SYNTH_ROW_OPEN_LIST, SYNTH_ROW_PICK_MATERIAL};
        assert_eq!(
            resolve_menu_entry(
                MenuKind::SynthLauncher,
                SYNTH_ROW_PICK_MATERIAL,
                false,
                false
            ),
            MenuDispatch::OpenSubmenu(MenuKind::Items)
        );
        assert_eq!(
            resolve_menu_entry(MenuKind::SynthLauncher, SYNTH_ROW_OPEN_LIST, false, false),
            MenuDispatch::OpenSubmenu(MenuKind::CraftList)
        );
    }

    #[test]
    fn missions_row_opens_the_battery_list() {
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Missions", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::MissionList)
        );
    }

    #[test]
    fn region_info_row_drills_the_four_mode_window() {
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Region Info", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::RegionInfo { mode: 0 })
        );
    }

    #[test]
    fn edit_macros_row_drills_the_mcrmenu_launcher() {
        // Ungated page-2 arm FFXiMain.dll retail-2026-09 RVA 0x14E8BD name-opens
        // `menu    mcrmenu` (vault main_menu.md §34); its two rows drill the
        // §41 windows bead .19.1 built.
        use kuluu_render::hud::menu::{MACRO_ROW_MACRO_RESULT, MACRO_ROW_SELECT_MACRO};
        assert_eq!(
            resolve_menu_entry(
                MenuKind::Root,
                kuluu_render::hud::menu::ROOT_MACRO_PALETTE,
                false,
                false,
            ),
            MenuDispatch::OpenSubmenu(MenuKind::MacrosPalette)
        );
        assert_eq!(
            resolve_menu_entry(
                MenuKind::MacrosPalette,
                MACRO_ROW_SELECT_MACRO,
                false,
                false
            ),
            MenuDispatch::OpenSubmenu(MenuKind::MacroSelect)
        );
        assert_eq!(
            resolve_menu_entry(
                MenuKind::MacrosPalette,
                MACRO_ROW_MACRO_RESULT,
                false,
                false
            ),
            MenuDispatch::OpenSubmenu(MenuKind::MacroResult)
        );
    }

    /// The Map row drills `myroom` lawfully (§40) and the fourteen home-view
    /// rows answer through §53's corrected select family (handler
    /// FFXiMain.dll retail-2026-09 RVA 0x145460 jump table .rdata 0x1455BC):
    /// container rows
    /// open their LSB bags (id1's `menu    storage`/`storage2` arms and the
    /// locker-family widgets), Gardening/Layout hand their literals to the
    /// runner, Change Jobs drills `jobchang`, Delivery Box opens the incoming
    /// delivery box over c2s 0x04D PBX (`mogpost`'s protocol); only the still-unbuilt
    /// retail arms (Remodel→`roomlist`, the Mog House door) stay on NotImplemented
    /// until their beads.
    #[test]
    fn home_view_rows_resolve_through_the_select_family() {
        use ffxi_proto::map::container as c;
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Map", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::Map)
        );
        for (label, want) in [
            (
                "M.Ward.",
                MenuDispatch::BrowseBag {
                    container: c::LOC_WARDROBE,
                },
            ),
            (
                "Mog Safe",
                MenuDispatch::BrowseBag {
                    container: c::LOC_MOGSAFE,
                },
            ),
            (
                "Storage",
                MenuDispatch::BrowseBag {
                    container: c::LOC_STORAGE,
                },
            ),
            (
                "Mog Locker",
                MenuDispatch::BrowseBag {
                    container: c::LOC_MOGLOCKER,
                },
            ),
            (
                "Satchel",
                MenuDispatch::BrowseBag {
                    container: c::LOC_MOGSATCHEL,
                },
            ),
            (
                "Mog Case",
                MenuDispatch::BrowseBag {
                    container: c::LOC_MOGCASE,
                },
            ),
            (
                "Mog Sack",
                MenuDispatch::BrowseBag {
                    container: c::LOC_MOGSACK,
                },
            ),
            (
                "Safe 2",
                MenuDispatch::BrowseBag {
                    container: c::LOC_MOGSAFE2,
                },
            ),
            ("Gardening", MenuDispatch::HomeTextCommand("/garden")),
            ("Layout", MenuDispatch::HomeTextCommand("/layout")),
            (
                "Change Jobs",
                MenuDispatch::OpenSubmenu(MenuKind::JobChange),
            ),
            ("Open Mog House", MenuDispatch::MogHouseDoor),
            ("Delivery Box", MenuDispatch::OpenDeliveryBox),
            ("Remodel", MenuDispatch::OpenSubmenu(MenuKind::RoomList)),
        ] {
            assert_eq!(
                resolve_menu_entry(MenuKind::HomeView, label, false, false),
                want,
                "{label}"
            );
        }
    }

    /// `jobchang`'s two DAT widgets (catalog tokens 65 "Main Job" / 66
    /// "Support Job", vault main_menu.md §53) drill their roster-built pick
    /// lists; each side commits its own c2s 0x100 index.
    #[test]
    fn job_change_rows_drill_their_pick_sides() {
        assert_eq!(
            resolve_menu_entry(MenuKind::JobChange, "Main Job", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::JobPick { main_pick: true })
        );
        assert_eq!(
            resolve_menu_entry(MenuKind::JobChange, "Support Job", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::JobPick { main_pick: false })
        );
    }

    /// Boilerplate-free confirm rig for child-window rows: runs the shared
    /// keyboard/mouse seam on `stack` with its own scratch writers.
    fn confirm_row(
        stack: &mut MenuStack,
        scene_state: &mut SceneState,
        cmd_tx: &Sender<AgentCommand>,
        item_bag: &mut kuluu_render::hud::item_screen::ItemScreenContainer,
    ) -> Option<InputMode> {
        let mut dynamic = kuluu_render::hud::menu::DynamicMenu::default();
        confirm_row_dynamic(stack, scene_state, cmd_tx, &mut dynamic, item_bag)
    }

    /// `conf3win` ids 3/4 (index 2/3) are the window-local pair: select writes only
    /// `(state_key, obj_off)` into `config_window_choices`, marks that row on the next
    /// build, flips when the partner row is taken, and never touches the option store
    /// or the wire (arms .text FFXiMain.dll retail-2026-09 RVA 0x19aa85/0x19aa92;
    /// vault main_menu.md §59 addendum 9; bead kuluu-o9qt7.20.1.2).
    #[test]
    fn conf3win_window_local_pair_flips_on_the_object_not_the_store() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows};
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf3win".to_string(),
            (0..15u32).map(|i| (800 + i, format!("c3_{i}"))).collect(),
        );
        let mut dynamic = kuluu_render::hud::menu::DynamicMenu::default();
        dynamic.rows = config_window_rows(
            "conf3win",
            &table,
            &dynamic.config_option_values,
            None,
            &dynamic.config_window_choices,
        );
        assert!(!dynamic.rows[2].label.starts_with('\u{bb}'));
        let mut stack = MenuStack::root();
        // Page index 4 of RETAIL_CONFIG_PAGE_WINDOWS is `conf3win`.
        stack.push(MenuKind::RetailConfigPage(4));
        let mut scene_state = SceneState::default();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(8);
        let mut bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();

        stack.current_mut().unwrap().cursor = 2; // select id 3
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            stack.levels.len(),
            2,
            "the window keeps retail's open window"
        );
        assert_eq!(
            dynamic.config_window_choices.get(&(4u8, 0x38u16)).copied(),
            Some(0)
        );

        // The partner row flips the same object field.
        stack.current_mut().unwrap().cursor = 3; // select id 4
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic.config_window_choices.get(&(4u8, 0x38u16)).copied(),
            Some(1)
        );

        // Marks follow the stored choice; nothing landed in the option store or wire.
        let rows = config_window_rows(
            "conf3win",
            &table,
            &dynamic.config_option_values,
            None,
            &dynamic.config_window_choices,
        );
        assert_eq!(rows[2].label, "c3_2");
        assert_eq!(rows[3].label, "\u{bb} c3_3");
        assert!(dynamic.config_option_values.is_empty());
        assert!(
            cmd_rx.try_recv().is_err(),
            "window-local writes never ride the wire"
        );
    }

    /// `conf6win` row 10 (index 9) is the dialog arm (.text FFXiMain.dll
    /// retail-2026-09 RVA 0x19B309): select pushes the Yes/No prompt (opens on No),
    /// No leaves the option store untouched, and Yes runs both panels' factory reset —
    /// options 0x4A..=0x69 and 0x6A..=0x89 rewritten from the templates in
    /// `key_assign_reset_defaults` (vault main_menu.md §59 addenda 7–8; bead
    /// kuluu-o9qt7.20.1.1). Nothing of this rides the wire.
    #[test]
    fn conf6win_dialog_row_asks_first_and_only_yes_factory_resets() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows};
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf6win".to_string(),
            (0..15u32).map(|i| (700 + i, format!("c6_{i}"))).collect(),
        );
        let mut dynamic = kuluu_render::hud::menu::DynamicMenu::default();
        for id in 0x4Au32..=0x89 {
            dynamic.config_option_values.insert(id, -7);
        }
        dynamic.rows = config_window_rows(
            "conf6win",
            &table,
            &dynamic.config_option_values,
            None,
            &std::collections::BTreeMap::new(),
        );
        let mut stack = MenuStack::root();
        // Page index 5 of RETAIL_CONFIG_PAGE_WINDOWS is `conf6win`.
        stack.push(MenuKind::RetailConfigPage(5));
        let mut scene_state = SceneState::default();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(8);
        let mut bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();

        // Select id 10: the prompt goes up with the cursor on No and nothing is written.
        stack.current_mut().unwrap().cursor = 9;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        {
            let level = stack.current().expect("reset prompt pushed");
            assert!(
                matches!(level.kind, MenuKind::KeyAssignResetPrompt),
                "conf6win row 10 must raise the reset prompt, got {:?}",
                level.kind
            );
            assert_eq!(
                level.cursor,
                kuluu_render::hud::menu::DROP_CONFIRM_DEFAULT,
                "retail's dialog callback defaults to the negative answer"
            );
        }
        // Answer No: dismissed, both option ranges keep their old values.
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(stack.levels.len(), 2, "No returns to the conf6win page");
        assert!(dynamic
            .config_option_values
            .values()
            .all(|&value| value == -7));

        // Answer Yes: every option in both panels' ranges takes its template default.
        stack.current_mut().unwrap().cursor = 9;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        stack.current_mut().unwrap().cursor = 0; // Yes
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(stack.levels.len(), 2, "the answer dismisses the prompt");
        for (option_id, value) in kuluu_render::hud::menu::key_assign_reset_defaults() {
            assert_eq!(
                dynamic.config_option_values.get(&option_id),
                Some(&value),
                "option {option_id:#x} after factory reset"
            );
        }
        assert!(
            cmd_rx.try_recv().is_err(),
            "the store law is client-local — no wire from a config reset"
        );
    }

    /// Same seam with caller-built dynamic rows — used for the Config family,
    /// whose rows come from a synthetic DAT table in these tests.
    fn confirm_row_dynamic(
        stack: &mut MenuStack,
        scene_state: &mut SceneState,
        cmd_tx: &Sender<AgentCommand>,
        dynamic: &mut kuluu_render::hud::menu::DynamicMenu,
        item_bag: &mut kuluu_render::hud::item_screen::ItemScreenContainer,
    ) -> Option<InputMode> {
        use crate::keybinds_store::KeybindsStore;
        let mut bindings = Bindings::default();
        let mut keybinds_state = KeybindsStateRes {
            store: KeybindsStore::new(std::env::temp_dir().join("kuluu-myroom-seam-tests.json")),
            persisted: Default::default(),
        };
        let mut graphics = kuluu_render::GraphicsSettings::default();
        let mut hud_panels = Default::default();
        let mut net_status = Default::default();
        let mut audio_mute = Default::default();
        let mut vana_clock = kuluu_render::vana_time::VanaClock::anchored_at_hour(12.0);
        let mut vana_clock_visible = Default::default();
        let surface = crate::view_native::command_surface::CommandSurface::default();
        confirm_menu_at_cursor(
            &mut bindings,
            stack,
            scene_state,
            cmd_tx,
            &mut keybinds_state,
            &mut graphics,
            &mut hud_panels,
            &mut net_status,
            &mut audio_mute,
            &mut vana_clock,
            &mut vana_clock_visible,
            dynamic,
            None,
            kuluu_snapshot::Vec3::default(),
            &surface,
            item_bag,
        )
    }

    fn stack_home_view_with_cursor(stack: &mut MenuStack, label: &str) {
        use kuluu_render::hud::menu::HOME_VIEW_ROWS;
        stack.push(MenuKind::HomeView);
        let row = HOME_VIEW_ROWS
            .iter()
            .position(|e| *e == label)
            .expect("home-view row");
        stack.current_mut().unwrap().cursor = row;
    }

    /// myroom reopened alone after layout mode closes on a text-command row as its handler closes it
    /// by name, with no main menu under it to fall back to.
    #[test]
    fn a_lone_myroom_closes_on_a_text_command_row() {
        let mut stack = MenuStack::default();
        stack_home_view_with_cursor(&mut stack, "Gardening");
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        let next = confirm_row(&mut stack, &mut scene_state, &cmd_tx, &mut item_bag);
        assert!(matches!(next, Some(InputMode::World)), "{next:?}");
    }

    /// Confirm-time law: the Storage row (id1's `menu    storage` arm, holder
    /// method FFXiMain.dll retail-2026-09 RVA 0x145C70(0)) opens kuluu's Items
    /// window on the LSB storage bag — the same seam a Mog Menu storage row uses.
    #[test]
    fn storage_row_confirm_opens_items_window_on_the_storage_bag() {
        use ffxi_proto::map::container as c;
        let mut stack = MenuStack::root();
        stack_home_view_with_cursor(&mut stack, "Storage");
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        confirm_row(&mut stack, &mut scene_state, &cmd_tx, &mut item_bag);
        assert_eq!(stack.current().map(|l| l.kind), Some(MenuKind::Items));
        assert_eq!(item_bag.0, c::LOC_STORAGE);
        // The home view stays open below, like the retail window pair.
        let kinds: Vec<MenuKind> = stack.levels.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![MenuKind::Root, MenuKind::HomeView, MenuKind::Items]
        );
    }

    /// Confirm-time law for bead kuluu-o9qt7.13.4: the Delivery Box row (select id 2,
    /// holder method .text FFXiMain.dll retail-2026-09 RVA 0x145BA0 name-opening
    /// `menu    mogpost` at .text 0x145BAB) queues exactly one PBX open — the incoming
    /// box, whose Work→Check list is what that window shows (kuluu-session/src/
    /// delivery_box.rs; vault main_menu.md §53). The panel level stays until the
    /// server-owned screen takes the display through `delivery_mode_sync_system`.
    #[test]
    fn delivery_box_row_opens_the_incoming_box_over_pbx() {
        let mut stack = MenuStack::root();
        stack_home_view_with_cursor(&mut stack, kuluu_render::hud::menu::HOME_ROW_DELIVERY_BOX);
        let mut scene_state = SceneState::default();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        confirm_row(&mut stack, &mut scene_state, &cmd_tx, &mut item_bag);
        assert_eq!(
            cmd_rx
                .try_recv()
                .expect("row must queue exactly one PBX open"),
            AgentCommand::DeliveryBox {
                op: kuluu_session::state::DeliveryBoxOp::PostOpen
            }
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "one select is one request — retail sends no second PBX op here"
        );
    }

    /// Confirm-time law for bead kuluu-o9qt7.13.6/.13.6.1: one press on the door row (select id 6 of
    /// handler FFXiMain.dll retail-2026-09 `.text 0x145460`, arm @`.text 0x14556C`) queues exactly
    /// one MYROOM_IS door select. Which Kind that is follows the client's own door byte (Open for a
    /// shut house, Close for an open one — `codec::moghouse_door_packet`), because LSB implements
    /// neither kind and ships nothing that could acknowledge it
    /// (vendor/server/src/map/packets/c2s/0x0cb_myroom_is.cpp; vault main_menu.md §59 addendum 24).
    /// A re-select queues a second such packet — retail re-sends too. The panel level stays up: the
    /// retail arm returns without touching the window.
    #[test]
    fn mog_house_door_row_queues_exactly_one_door_select_per_press() {
        let mut stack = MenuStack::root();
        stack_home_view_with_cursor(&mut stack, kuluu_render::hud::menu::HOME_ROW_OPEN_MOG_HOUSE);
        let mut scene_state = SceneState::default();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        confirm_row(&mut stack, &mut scene_state, &cmd_tx, &mut item_bag);
        assert_eq!(
            cmd_rx
                .try_recv()
                .expect("door row must queue exactly one door select"),
            AgentCommand::MogHouseDoorSelect
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "one select is one MYROOM_IS packet — the arm sends no second kind"
        );
        confirm_row(&mut stack, &mut scene_state, &cmd_tx, &mut item_bag);
        assert_eq!(
            cmd_rx
                .try_recv()
                .expect("a re-select re-sends the door select"),
            AgentCommand::MogHouseDoorSelect
        );
        assert_eq!(stack.current().map(|l| l.kind), Some(MenuKind::HomeView));
    }

    /// Confirm-time law for bead kuluu-o9qt7.13.5: the myroom Remodel row opens
    /// `roomlist`; a style row raises the yes/no prompt (retail dialog service .text
    /// FFXiMain.dll retail-2026-09 0x210E30, opening on No) and only YES runs the
    /// sender wrapper .text 0x1D6AB0 — exactly one MYROOM_IS Kind=Remodel carrying that
    /// row's Param2 style word. NO dismisses without a packet; both answers land back
    /// on the roomlist. Row 0 — style word 0, caption "No changes" — never reaches
    /// this seam: its select arm only refreshes/blips (.text FFXiMain.dll retail-2026-09
    /// RVA 0x1D6B52 → .text 0x1D6B10).
    #[test]
    fn room_list_style_row_confirms_before_its_single_remodel_send() {
        use kuluu_render::hud::menu::{roomlist_rows, ConfigDatRows};
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        // Stand-in rows with the catalog shape; the install-gated pin
        // real_roomlist_caption_catalog_entries_match_the_dll_fetches (ffxi-dat)
        // holds the DAT side of the labels.
        table.by_root.insert(
            "roomlist".to_string(),
            vec![
                (450, "No changes".to_string()),
                (446, "San d'Orian style".to_string()),
                (447, "Bastokan style".to_string()),
                (448, "Windurstian style".to_string()),
                (449, "Mog Patio".to_string()),
            ],
        );
        let mut dynamic = kuluu_render::hud::menu::DynamicMenu {
            rows: roomlist_rows(&table),
            ..Default::default()
        };
        assert_eq!(dynamic.rows.len(), 5);

        let mut stack = MenuStack::root();
        stack.push(MenuKind::RoomList);
        let mut scene_state = SceneState::default();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(8);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();

        // Row 0 is display-only (style word 0): select answers but sends nothing.
        stack.current_mut().unwrap().cursor = 0;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut item_bag,
        );
        assert!(cmd_rx.try_recv().is_err(), "row 0 never reaches the wire");

        // Row 4 = Mog Patio (618) is gated before it answers anything: retail keeps the row
        // dim and reduces its select to the error bell until the character owns key item
        // 3051 (.text FFXiMain.dll retail-2026-09 0x1D6B63), so no prompt opens and nothing
        // reaches the wire.
        stack.current_mut().unwrap().cursor = 4;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut item_bag,
        );
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::RoomList),
            "without the patio bell the row answers nothing"
        );

        // Earn the bell through the s2c 0x055 have store and the same select works.
        scene_state
            .snapshot
            .key_items
            .push(kuluu_render::hud::menu::PATIO_BELL_KEY_ITEM);
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut item_bag,
        );
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::RoomListConfirm { style: 618 }),
            "the prompt carries the row's Param2 style word"
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "opening the prompt sends nothing"
        );

        // …and No dismisses it back onto the roomlist with no packet.
        stack.current_mut().unwrap().cursor = kuluu_render::hud::menu::DROP_CONFIRM_DEFAULT;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut item_bag,
        );
        assert!(cmd_rx.try_recv().is_err(), "No writes nothing");
        assert_eq!(stack.current().map(|l| l.kind), Some(MenuKind::RoomList));

        // Yes runs the sender: exactly one MYROOM_IS Kind=Remodel, Param2 618.
        stack.current_mut().unwrap().cursor = 4;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut item_bag,
        );
        stack.current_mut().unwrap().cursor = 0; // row 0 = Yes
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut item_bag,
        );
        assert_eq!(
            cmd_rx.try_recv().expect("Yes sends exactly one remodel"),
            AgentCommand::MogHouseRemodel { style: 618 }
        );
        assert!(cmd_rx.try_recv().is_err(), "one confirm is one packet");
        assert_eq!(stack.current().map(|l| l.kind), Some(MenuKind::RoomList));
    }

    /// Confirm-time law: the Gardening row runs its literal through the runner
    /// and then closes myroom by name (arms 0x145541/0x15E910 tail; §53) — kuluu
    /// pops the home-view level, landing back on Root.
    #[test]
    fn gardening_row_runs_its_literal_and_closes_the_panel() {
        let mut stack = MenuStack::root();
        stack_home_view_with_cursor(&mut stack, "Gardening");
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        confirm_row(&mut stack, &mut scene_state, &cmd_tx, &mut item_bag);
        assert_eq!(stack.current().map(|l| l.kind), Some(MenuKind::Root));
    }

    /// A job pick commits exactly one c2s 0x100 MYROOM_JOB side: the picked id
    /// rides its own index, the other side stays zero ("no change" — LSB skips
    /// validating zero indices per vendor/server/src/map/packets/c2s/
    /// 0x100_myroom_job.cpp; vault main_menu.md §53).
    #[test]
    fn home_job_picks_send_their_side_of_change_job() {
        use kuluu_render::hud::menu::DynamicMenuAction;
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut scene_state = SceneState::default();
        dispatch_dynamic_menu_action(
            DynamicMenuAction::HomeJobRow {
                job_id: 6,
                as_main: true,
            },
            None,
            kuluu_snapshot::Vec3::default(),
            &[],
            &cmd_tx,
            &mut scene_state,
            false,
        );
        assert_eq!(
            cmd_rx.try_recv().expect("main pick must send"),
            AgentCommand::ChangeJob {
                main_job: Some(6),
                sub_job: None
            }
        );
        dispatch_dynamic_menu_action(
            DynamicMenuAction::HomeJobRow {
                job_id: 3,
                as_main: false,
            },
            None,
            kuluu_snapshot::Vec3::default(),
            &[],
            &cmd_tx,
            &mut scene_state,
            false,
        );
        assert_eq!(
            cmd_rx.try_recv().expect("sub pick must send"),
            AgentCommand::ChangeJob {
                main_job: None,
                sub_job: Some(3)
            }
        );
    }

    #[test]
    fn config_row_drills_the_retail_hub() {
        // Ungated page-2 arm FFXiMain.dll retail-2026-09 RVA 0x14E8D9
        // name-opens `menu    configwi` (vault main_menu.md §35). The hub's own
        // rows are DAT widgets read at runtime (bead kuluu-o9qt7.33); their
        // drills are pinned in confirm-time by config_hub_confirm_pushes_the_page_windows.
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Config", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::RetailConfig)
        );
    }

    /// Confirm-time law for the DAT-driven Config family (bead kuluu-o9qt7.33):
    /// hub row i pushes its own page window — the twelve jump-table branches of
    /// FFXiMain.dll retail-2026-09 RVA 0x196770 (.text 0x1968A4; vault main_menu.md §42)
    /// — while a thirteenth widget falls to the handler's default tail and answers as an
    /// option row instead (vault main_menu.md §57).
    #[test]
    fn config_hub_confirm_pushes_the_page_windows() {
        use kuluu_render::hud::menu::{
            config_window_rows, ConfigDatRows, DynamicMenu, RETAIL_CONFIG_PAGE_WINDOWS,
        };
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "configwi".to_string(),
            (0..13u32).map(|i| (200 + i, format!("hub{i}"))).collect(),
        );
        let mut dynamic = DynamicMenu {
            rows: config_window_rows(
                "configwi",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..DynamicMenu::default()
        };
        for page in 0..RETAIL_CONFIG_PAGE_WINDOWS.len() {
            let mut stack = MenuStack::root();
            stack.push(MenuKind::RetailConfig);
            stack.current_mut().unwrap().cursor = page;
            let mut scene_state = SceneState::default();
            let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
            let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
            confirm_row_dynamic(
                &mut stack,
                &mut scene_state,
                &cmd_tx,
                &mut dynamic,
                &mut item_bag,
            );
            assert_eq!(
                stack.current().map(|l| l.kind),
                Some(MenuKind::RetailConfigPage(page as u8)),
                "hub row {page} must push its own page window"
            );
        }
        // The widget past the twelve branches (GAMEPAD Config launcher) has no drill:
        // it stays put and answers with the option line instead.
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfig);
        stack.current_mut().unwrap().cursor = 12;
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut item_bag,
        );
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::RetailConfig)
        );
        let toast = &scene_state.local_toasts.last().expect("answer line").text;
        assert!(
            toast.contains("hub12") && toast.contains("not implemented"),
            "{toast}"
        );
    }

    /// `conf5m`'s select law (handler FFXiMain.dll retail-2026-09 RVA 0x199240, arms
    /// .text 0x199264/0x19927A/0x199290): its first widget name-opens `conf5win` (sub 0,
    /// the shared-settings page), the second `conf5w1`, and the third falls through to
    /// `conf5w2`. A leaf option row answers with the standard line WITHOUT closing
    /// its window — retail's leaf selects carry no window event, and their store writes
    /// are plain option-store latches.
    #[test]
    fn config_launcher_and_option_rows_follow_their_arms() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows, DynamicMenu};
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf5m".to_string(),
            vec![
                (901, "shared".to_string()),
                (902, "individual".to_string()),
                (903, "other".to_string()),
            ],
        );
        let mut launcher = DynamicMenu {
            rows: config_window_rows(
                "conf5m",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..DynamicMenu::default()
        };
        for (cursor, sub) in [(0usize, 0u8), (1, 1), (2, 2)] {
            let mut stack = MenuStack::root();
            stack.push(MenuKind::RetailConfigPage(3));
            stack.current_mut().unwrap().cursor = cursor;
            let mut scene_state = SceneState::default();
            let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
            let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
            confirm_row_dynamic(
                &mut stack,
                &mut scene_state,
                &cmd_tx,
                &mut launcher,
                &mut item_bag,
            );
            assert_eq!(
                stack.current().map(|l| l.kind),
                Some(MenuKind::RetailConfigSub(sub)),
                "conf5m widget {cursor} must name-open sub {sub}"
            );
        }
        // `textcol2` is the placeholder leaf window, and permanently so: its
        // vtable VA 0x10337678 answers no window command at all — slot +0x18
        // holds the generic `ret 8` stub FFXiMain.dll retail-2026-09 RVA
        // 0x1012AC60 (vault main_menu.md §59 addendum 5). Its rows therefore keep
        // the §46 option-record fallback forever (`conftxtc` earned its companion
        // arms this cycle; `conf7` before it).
        table.by_root.insert(
            "textcol2".to_string(),
            vec![(500, "o1".to_string()), (501, "o2".to_string())],
        );
        let mut page = DynamicMenu {
            rows: config_window_rows(
                "textcol2",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..DynamicMenu::default()
        };
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigSub(
            kuluu_render::hud::menu::CONFIG_SUB_TEXTCOL2,
        ));
        stack.current_mut().unwrap().cursor = 1;
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut page,
            &mut item_bag,
        );
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::RetailConfigSub(
                kuluu_render::hud::menu::CONFIG_SUB_TEXTCOL2
            )),
            "option-row selects must keep the page window open"
        );
        let toast = &scene_state.local_toasts.last().expect("answer line").text;
        assert!(
            toast.contains("o2") && toast.contains("not implemented"),
            "{toast}"
        );
    }

    /// The companion-drill seam (vault main_menu.md §59 addendum 5): a Text
    /// Color row's select pushes the window retail leaves focused (`textcol1`,
    /// sub index CONFIG_SUB_TEXTCOL1) and records the mode dword its handler
    /// wrote into that companion object; nothing goes on the wire.
    #[test]
    fn conf_companion_row_pushes_the_focused_window_with_its_mode() {
        use kuluu_render::hud::menu::{
            config_page_arms, config_window_rows, ConfigRowArm, CONFIG_SUB_TEXTCOL1,
            CONFTXT_TEXT_OPENS,
        };
        let mut table = kuluu_render::hud::menu::ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        let arms = config_page_arms("conftxtc").unwrap_or_default();
        // One DAT row per handler select id (the install pin on this table's
        // length lives in kuluu-render).
        table.by_root.insert(
            "conftxtc".to_string(),
            (0..arms.len())
                .map(|i| (900 + i as u32, format!("txt{i}")))
                .collect(),
        );
        assert!(matches!(
            &arms[2],
            ConfigRowArm::Companion { mode: 2, opens } if *opens == CONFTXT_TEXT_OPENS
        ));
        let mut dynamic = kuluu_render::hud::menu::DynamicMenu {
            rows: config_window_rows(
                "conftxtc",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..Default::default()
        };
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigPage(2));
        stack.current_mut().unwrap().cursor = 2;
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut item_bag,
        );
        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::RetailConfigSub(CONFIG_SUB_TEXTCOL1)),
            "the row opens the companion retail leaves focused"
        );
        assert_eq!(
            dynamic.config_companion_modes.get(&CONFIG_SUB_TEXTCOL1),
            Some(&2),
            "retail writes the row index into the companion mode dword"
        );
    }

    /// `conf5win`'s radio law (bead kuluu-o9qt7.20.1): each row writes its
    /// jump-table (store id, value) pair into the client-side option store —
    /// silently, window open, nothing on the wire (LSB's c2s 0x0DC only ever
    /// accepts SAVE_CONF bits). The next row build marks the chosen one.
    #[test]
    fn conf5win_radio_rows_write_the_option_store_and_mark() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows, DynamicMenu};
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf5win".to_string(),
            (0..16u32).map(|i| (900 + i, format!("opt{i}"))).collect(),
        );
        let mut dynamic = DynamicMenu {
            rows: config_window_rows(
                "conf5win",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..DynamicMenu::default()
        };
        assert!(
            !dynamic.rows.iter().any(|r| r.label.starts_with('\u{bb}')),
            "unknown value renders unmarked"
        );

        // Row 9 is the arm (store 0x12, set value 3).
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigSub(0));
        stack.current_mut().unwrap().cursor = 9;
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut item_bag,
        );

        assert_eq!(
            stack.current().map(|l| l.kind),
            Some(MenuKind::RetailConfigSub(0)),
            "radio select keeps the window open"
        );
        assert_eq!(dynamic.config_option_values.get(&0x12), Some(&3));
        assert!(
            !scene_state
                .local_toasts
                .iter()
                .any(|t| t.text.contains("not implemented")),
            "the law'd row answers with a write, not the placeholder line"
        );

        let rows = config_window_rows(
            "conf5win",
            &table,
            &dynamic.config_option_values,
            dynamic.option_edit,
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(rows[9].label, "\u{bb} opt9");
        assert!(!rows[8].label.starts_with('\u{bb}'));
    }

    #[test]
    fn help_desk_row_drills_the_faqsub_panel() {
        // Page-2 arm FFXiMain.dll retail-2026-09 RVA 0x14E9C0 -> trampoline
        // 0x2174C0 name-opens `menu    faqsub` (vault main_menu.md §36); the
        // three destination rows answer until `.21.1` builds them.
        use kuluu_render::hud::menu::{FAQ_ROW_CONFIG, FAQ_ROW_MENTOR, FAQ_ROW_QA};
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Help Desk", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::HelpDeskPanel)
        );
        for label in [FAQ_ROW_QA, FAQ_ROW_MENTOR, FAQ_ROW_CONFIG] {
            assert!(matches!(
                resolve_menu_entry(MenuKind::HelpDeskPanel, label, false, false),
                MenuDispatch::NotImplemented(_)
            ));
        }
    }

    #[test]
    fn communication_row_drills_both_commen_list_widgets() {
        // Page-2 arm FFXiMain.dll retail-2026-09 RVA 0x14E970 name-opens
        // `menu    commenu` (vault main_menu.md §37); the DAT order is
        // windowps330 Friend List then windowps928 Emote List, and each row
        // drills its own list widget.
        use kuluu_render::hud::menu::{COMM_EMOTE_LIST, COMM_FRIEND_LIST};
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Communication", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::Communication)
        );
        assert_eq!(
            resolve_menu_entry(MenuKind::Communication, COMM_FRIEND_LIST, false, false),
            MenuDispatch::OpenSubmenu(MenuKind::FriendList)
        );
        assert_eq!(
            resolve_menu_entry(MenuKind::Communication, COMM_EMOTE_LIST, false, false),
            MenuDispatch::OpenSubmenu(MenuKind::EmoteList)
        );
    }

    #[test]
    fn unwired_root_entries_stay_not_implemented() {
        // Party is built (opens the prtyN window per vault main_menu.md §24), so
        // are Search (§26), Region Info (§29), Missions (§30) and View House —
        // the Mog Menu row (§32); everything else still answers
        // "Not supported yet".
        // Bazaar is built now (drills the stall picker, vault main_menu.md §33),
        // and so are Edit Macros (§34 launcher), Config (§35 hub) and Help Desk
        // (§36 panel): every Root entry resolves to a drill or a command —
        // nothing unwired. Assert it by scanning the whole root list.
        for label in kuluu_render::hud::menu::entries_for(MenuKind::Root, false) {
            assert!(
                !matches!(
                    resolve_menu_entry(MenuKind::Root, label, false, false),
                    MenuDispatch::NotImplemented(_)
                ),
                "{label} must resolve"
            );
        }
    }

    #[test]
    fn bazaar_row_drills_the_stall_picker() {
        // Ungated page-2 arm 0x14E8A1 name-open of `menu    bazaar` (vault
        // main_menu.md §33).
        assert_eq!(
            resolve_menu_entry(
                MenuKind::Root,
                kuluu_render::hud::menu::ROOT_SET_BAZAAR,
                false,
                false,
            ),
            MenuDispatch::OpenSubmenu(MenuKind::BazaarStalls)
        );
    }

    #[test]
    fn view_house_row_opens_the_mog_menu() {
        // Retail arm FFXiMain.dll retail-2026-09 RVA 0x14E8F5 name-opens
        // `menu    mogcont` (vault main_menu.md §32); kuluu routes the same
        // open through AgentCommand::OpenMogMenu at confirm time.
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "View House", false, false),
            MenuDispatch::OpenMogMenu
        );
    }

    /// The right-pane preview (`menu::root_child_kind`) and the drill dispatch
    /// share one Root → submenu mapping; pin that they can't drift apart.
    #[test]
    fn root_drill_matches_preview_child_kind() {
        use kuluu_render::hud::menu::{self, ROOT_CURRENT_TIME, ROOT_LOG_OUT, ROOT_SHUT_DOWN};
        for &label in menu::root_entries() {
            // Log Out / Shut Down open the yes/no prompt and Current Time, Key
            // Items and Quests run text commands - none of them has a right-pane
            // preview (vault main_menu.md §31 for the quest arm).
            // View House dispatches OpenMogMenu (dialog surface, no submenu),
            // like the power/text rows it takes no right-pane preview.
            if matches!(
                label,
                ROOT_LOG_OUT
                    | ROOT_SHUT_DOWN
                    | ROOT_CURRENT_TIME
                    | "Key Items"
                    | "Quests"
                    | "View House"
            ) {
                continue;
            }
            match (
                resolve_menu_entry(MenuKind::Root, label, false, false),
                menu::root_child_kind(label, false, false),
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
        use super::super::slash_apply::activate_current_time;
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

    /// Retail's Current Time row hands the runner its literal string
    /// (FFXiMain.dll retail-2026-09 RVA 0x14E98C, .data 0x371AB8); kuluu must
    /// route through the same text-command seam, not a bespoke intercept.
    #[test]
    fn current_time_row_runs_the_retail_clock_literal() {
        use kuluu_render::hud::menu::ROOT_CURRENT_TIME;
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, ROOT_CURRENT_TIME, false, false),
            MenuDispatch::TextCommand("/clock"),
        );
    }

    /// The runner seam (vault main_menu.md §12, bead kuluu-o9qt7.28): row literals go through
    /// the same `parse_slash` typed input uses - "/clock" reaches the widget toggle + time lines;
    /// an unknown word answers with the parser's own unknown-command line (the shim-specific
    /// refusal wording retired with the unification).
    #[test]
    fn text_command_runner_routes_clock_and_refuses_unknown() {
        use kuluu_render::hud::vana_clock::VANA_TIME_LINE_PREFIX;
        use kuluu_render::vana_time::VanaClock;

        let clock = VanaClock::anchored_at_hour(12.0);
        let mut visible = kuluu_render::hud::vana_clock::VanaClockVisible::default();
        let mut scene_state = SceneState::default();
        let mut stack = MenuStack::root();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let surface = crate::view_native::command_surface::CommandSurface::default();
        seam_call(
            "/clock",
            &surface,
            &mut stack,
            &clock,
            &mut visible,
            &mut scene_state,
            &cmd_tx,
        );
        // The widget's default is shown, so the retail toggle hides it.
        assert!(!visible.0, "the widget flipped off from its shown default");
        assert!(
            scene_state
                .local_toasts
                .iter()
                .any(|l| l.text.starts_with(VANA_TIME_LINE_PREFIX)),
            "{}",
            scene_state.local_toasts.len()
        );
        seam_call(
            "/nope",
            &surface,
            &mut stack,
            &clock,
            &mut visible,
            &mut scene_state,
            &cmd_tx,
        );
        assert!(scene_state
            .local_toasts
            .iter()
            .any(|l| { l.text.contains("unknown command") && l.text.contains("/nope") }));
    }

    /// The seam's arguments ride the snapshot and shared primitives (bead kuluu-o9qt7.28), so a
    /// test can drive it without any Bevy scaffolding.
    #[allow(clippy::too_many_arguments)]
    fn seam_call(
        cmd: &str,
        surface: &crate::view_native::command_surface::CommandSurface,
        stack: &mut MenuStack,
        clock: &kuluu_render::vana_time::VanaClock,
        visible: &mut kuluu_render::hud::vana_clock::VanaClockVisible,
        scene_state: &mut SceneState,
        cmd_tx: &Sender<AgentCommand>,
    ) {
        apply_menu_text_command(
            cmd,
            surface,
            stack,
            clock,
            visible,
            scene_state,
            cmd_tx,
            kuluu_snapshot::Vec3::default(),
            None,
            &mut kuluu_render::hud::menu::DynamicMenu::default(),
        );
    }

    /// Retail's Key Items row runs "/keyitem" (FFXiMain.dll retail-2026-09 RVA
    /// 0x14E855, .data 0x371B40; vault §12 id 0x4C): the dispatch hands over the
    /// literal and the runner opens the key-item list window.
    #[test]
    fn key_items_row_runs_the_retail_keyitem_literal() {
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Key Items", false, false),
            MenuDispatch::TextCommand("/keyitem"),
        );

        let clock = kuluu_render::vana_time::VanaClock::anchored_at_hour(12.0);
        let mut visible = kuluu_render::hud::vana_clock::VanaClockVisible::default();
        let mut scene_state = SceneState::default();
        let mut stack = MenuStack::root();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let surface = crate::view_native::command_surface::CommandSurface::default();
        seam_call(
            "/keyitem",
            &surface,
            &mut stack,
            &clock,
            &mut visible,
            &mut scene_state,
            &cmd_tx,
        );
        assert_eq!(stack.current().map(|l| l.kind), Some(MenuKind::KeyItems));
    }

    /// Retail's Quests row runs "/quest" (FFXiMain.dll retail-2026-09 RVA
    /// 0x14E889, .data 0x371B24; vault main_menu.md §31 id-table entry 0x4D →
    /// handler RVA 0x7D240): the dispatch hands over the literal and the runner opens the
    /// quest00-stand-in list.
    #[test]
    fn quests_row_runs_the_retail_quest_literal() {
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Quests", false, false),
            MenuDispatch::TextCommand("/quest"),
        );

        let clock = kuluu_render::vana_time::VanaClock::anchored_at_hour(12.0);
        let mut visible = kuluu_render::hud::vana_clock::VanaClockVisible::default();
        let mut scene_state = SceneState::default();
        let mut stack = MenuStack::root();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let surface = crate::view_native::command_surface::CommandSurface::default();
        seam_call(
            "/quest",
            &surface,
            &mut stack,
            &clock,
            &mut visible,
            &mut scene_state,
            &cmd_tx,
        );
        assert_eq!(stack.current().map(|l| l.kind), Some(MenuKind::QuestList));
    }

    /// The unification (bead kuluu-o9qt7.28): the seam runs `parse_slash`, so a retail alias from
    /// the install's command table reaches its handler too, and both spellings of the clock parse
    /// to one outcome - state the same primitive applies (`set_current_time`).
    #[test]
    fn menu_text_command_shares_the_typed_parser_and_aliases() {
        use ffxi_dat::main_dll::{ClientCommand, CommandTable};

        let surface = crate::view_native::command_surface::CommandSurface::new(
            CommandTable::from_entries(vec![
                ClientCommand {
                    name: "clock".to_owned(),
                    id: 0x3f,
                    flags: 0,
                },
                ClientCommand {
                    name: "cl".to_owned(),
                    id: 0x3f,
                    flags: 0,
                },
            ]),
        );

        let clock = kuluu_render::vana_time::VanaClock::anchored_at_hour(12.0);
        let mut visible = kuluu_render::hud::vana_clock::VanaClockVisible::default();
        let mut scene_state = SceneState::default();
        let mut stack = MenuStack::root();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        seam_call(
            "/cl",
            &surface,
            &mut stack,
            &clock,
            &mut visible,
            &mut scene_state,
            &cmd_tx,
        );
        // Shown by default: flipping off proves the alias landed on the clock handler.
        assert!(
            !visible.0,
            "the alias reached the clock handler through parse_slash"
        );

        // Both spellings produce exactly one typed outcome off the same snapshot args.
        let gate = kuluu_render::fishing_spot::gate_from_snapshot(&scene_state.snapshot);
        for spelling in ["/clock", "/cl"] {
            let out = parse_slash(
                spelling,
                &surface,
                &scene_state.snapshot.entities,
                kuluu_snapshot::Vec3::default(),
                None,
                scene_state.snapshot.zone_id,
                scene_state.snapshot.self_char_id,
                &scene_state.snapshot.party,
                gate,
                None,
                scene_state.snapshot.self_pet_targid,
            );
            assert!(
                matches!(out, SlashOutcome::SetVanaClock(None)),
                "{spelling}: the seam's source of truth is one outcome, got {out:?}"
            );
        }
    }

    /// End to end (bead kuluu-o9qt7.28): confirming the Root Current Time row - the seam both
    /// the keyboard and mouse paths share - lands the typed `/clock` state: widget flips, both
    /// time lines print, menu stays open on the same level.
    #[test]
    fn current_time_row_confirm_matches_typed_clock() {
        use crate::keybinds_store::KeybindsStore;
        use kuluu_render::hud::menu::ROOT_CURRENT_TIME;

        let mut bindings = Bindings::default();
        // Root at cursor 0, like InputMode::Menu(MenuStack::root()).
        let mut stack = MenuStack::root();
        let row = kuluu_render::hud::menu::root_entries()
            .iter()
            .position(|e| *e == ROOT_CURRENT_TIME)
            .expect("Current Time is a root row");
        stack.current_mut().unwrap().cursor = row;
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel::<AgentCommand>(4);
        let mut keybinds_state = KeybindsStateRes {
            store: KeybindsStore::new(std::env::temp_dir().join("kuluu-clock-seam-tests.json")),
            persisted: Default::default(),
        };
        let mut graphics = kuluu_render::GraphicsSettings::default();
        let mut hud_panels = Default::default();
        let mut net_status = Default::default();
        let mut audio_mute = Default::default();
        let mut vana_clock = kuluu_render::vana_time::VanaClock::anchored_at_hour(12.0);
        let mut vana_clock_visible = Default::default();
        let mut dynamic = kuluu_render::hud::menu::DynamicMenu::default();
        let surface = crate::view_native::command_surface::CommandSurface::default();

        let mut item_bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        let next = confirm_menu_at_cursor(
            &mut bindings,
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut keybinds_state,
            &mut graphics,
            &mut hud_panels,
            &mut net_status,
            &mut audio_mute,
            &mut vana_clock,
            &mut vana_clock_visible,
            &mut dynamic,
            None,
            kuluu_snapshot::Vec3::default(),
            &surface,
            &mut item_bag,
        );

        assert!(next.is_none(), "the menu stays open");
        assert_eq!(stack.current().map(|l| l.kind), Some(MenuKind::Root));
        // Shown by default: flipping off is the toggle landing (bead kuluu-o9qt7.28).
        assert!(!vana_clock_visible.0, "widget flipped like typed /clock");
        use kuluu_render::hud::vana_clock::{EARTH_TIME_LINE_PREFIX, VANA_TIME_LINE_PREFIX};
        let lines: Vec<&str> = scene_state
            .local_toasts
            .iter()
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(lines.len(), 2, "both retail time lines print: {lines:?}");
        assert!(
            lines[0].starts_with(VANA_TIME_LINE_PREFIX)
                && lines[1].starts_with(EARTH_TIME_LINE_PREFIX),
            "{lines:?}"
        );
    }

    /// The live Status selector is statcom2 — its select handler (FFXiMain.dll
    /// retail-2026-09 RVA 0x148D00 over detail table 0x3716B8) answers every row,
    /// so the panel is interactive by default. Persona-passive silence (the ret-8
    /// stub at RVA 0x12AC60 covers the persona window alone) is an opt-in flip of
    /// the "Status Rows" Retail+ toggle.
    #[test]
    fn status_rows_active_by_default_with_persona_passive_option() {
        use kuluu_render::GraphicsSettings;

        let mut graphics = GraphicsSettings::default();
        assert!(
            graphics.interactive_status_rows,
            "retail statcom2 law: selector rows answer by default"
        );
        let mut scene_state = SceneState::default();
        assert!(handle_retail_plus_row(
            kuluu_render::hud::menu::RETAIL_STATUS_ROWS,
            &mut graphics,
            &mut scene_state,
        ));
        assert!(
            !graphics.interactive_status_rows,
            "first flip opts into the persona-passive panel"
        );
        handle_retail_plus_row(
            kuluu_render::hud::menu::RETAIL_STATUS_ROWS,
            &mut graphics,
            &mut scene_state,
        );
        assert!(
            graphics.interactive_status_rows,
            "flips back to answering rows"
        );
    }

    /// Retail's equipment window equips on every select case - no toggle-off branch in the
    /// command-event jump table (FFXiMain.dll retail-2026-09 RVA 0x1322B0 / 0x132608, vault
    /// main_menu.md §20) - so reselect-unequip is off by default and flips only through the
    /// Debug/Retail+ "Reselect Unequip" row.
    #[test]
    fn reselect_unequip_off_by_default_with_retail_plus_toggle() {
        use kuluu_render::GraphicsSettings;

        let mut graphics = GraphicsSettings::default();
        assert!(
            !graphics.reselect_unequip,
            "retail default: re-selecting the equipped item just re-equips"
        );
        let mut scene_state = SceneState::default();
        assert!(handle_retail_plus_row(
            kuluu_render::hud::menu::RETAIL_RESELECT_UNEQUIP,
            &mut graphics,
            &mut scene_state,
        ));
        assert!(graphics.reselect_unequip, "toggle flips on");
        handle_retail_plus_row(
            kuluu_render::hud::menu::RETAIL_RESELECT_UNEQUIP,
            &mut graphics,
            &mut scene_state,
        );
        assert!(!graphics.reselect_unequip, "flips back off");
    }

    /// Retail's Magic row is the two-window flow (vault main_menu.md §21): Root → mgcmenu picker →
    /// school-filtered `magic` list. The flat one-step list only exists with the Debug/Retail+
    /// "Flat Magic List" toggle, which is off by default and flips through that row.
    #[test]
    fn magic_drills_the_retail_picker_and_the_flat_toggle_moves_it() {
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Magic List", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::MagicGroups)
        );
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Magic List", true, false),
            MenuDispatch::OpenSubmenu(MenuKind::Magic)
        );

        let mut graphics = kuluu_render::GraphicsSettings::default();
        assert!(
            !graphics.magic_flat_list,
            "retail default: Magic goes through mgcmenu"
        );
        let mut scene_state = SceneState::default();
        assert!(handle_retail_plus_row(
            kuluu_render::hud::menu::RETAIL_MAGIC_FLAT_LIST,
            &mut graphics,
            &mut scene_state,
        ));
        assert!(graphics.magic_flat_list, "toggle flips on");
        handle_retail_plus_row(
            kuluu_render::hud::menu::RETAIL_MAGIC_FLAT_LIST,
            &mut graphics,
            &mut scene_state,
        );
        assert!(!graphics.magic_flat_list, "flips back off");
    }

    /// Picking a mgcmenu category drills to that school's spell list on the §21 numbers
    /// (display row 3 is Songs = 5, row 5 Summoning = 3).
    #[test]
    fn mgcmenu_category_select_opens_the_matching_spell_list() {
        for (label, school) in [
            ("White Magic", 1u8),
            ("Black Magic", 2),
            ("Songs", 5),
            ("Ninjutsu", 4),
            ("Summoning", 3),
            ("Blue Magic", 6),
            ("Geomancy", 7),
            ("Trust Magic", 8),
        ] {
            assert_eq!(
                resolve_menu_entry(MenuKind::MagicGroups, label, false, false),
                MenuDispatch::OpenSubmenu(MenuKind::MagicGroup(school)),
                "{label} should open school {school}",
            );
        }
    }

    /// The Abilities row follows the same two-window law (vault main_menu.md §23):
    /// Root → abimenu picker → group-filtered list. Kuluu's flat one-step ability
    /// list survives behind the Debug/Retail+ "Flat Abilities List" toggle, off by
    /// default like its magic twin.
    #[test]
    fn abilities_drill_the_retail_picker_and_the_flat_toggle_moves_it() {
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Abilities", false, false),
            MenuDispatch::OpenSubmenu(MenuKind::AbilitiesGroups)
        );
        assert_eq!(
            resolve_menu_entry(MenuKind::Root, "Abilities", false, true),
            MenuDispatch::OpenSubmenu(MenuKind::Abilities)
        );

        let mut graphics = kuluu_render::GraphicsSettings::default();
        assert!(
            !graphics.abilities_flat_list,
            "retail default: Abilities goes through abimenu"
        );
        let mut scene_state = SceneState::default();
        assert!(handle_retail_plus_row(
            kuluu_render::hud::menu::RETAIL_ABILITIES_FLAT_LIST,
            &mut graphics,
            &mut scene_state,
        ));
        assert!(graphics.abilities_flat_list, "toggle flips on");
        handle_retail_plus_row(
            kuluu_render::hud::menu::RETAIL_ABILITIES_FLAT_LIST,
            &mut graphics,
            &mut scene_state,
        );
        assert!(!graphics.abilities_flat_list, "flips back off");
    }

    /// Picking an abimenu category drills to that group's ability list; the picker
    /// rows pair positionally with `AbilityGroup::ALL` (the [I-pending] grounding
    /// recorded on bead kuluu-o9qt7.7 and vault main_menu.md §23).
    #[test]
    fn abimenu_category_select_opens_the_matching_ability_list() {
        use kuluu_render::hud::action_model::AbilityGroup;
        assert_eq!(
            kuluu_render::hud::menu::ABILITIES_CATEGORY_LABELS.len(),
            AbilityGroup::ALL.len()
        );
        for (idx, label) in kuluu_render::hud::menu::ABILITIES_CATEGORY_LABELS
            .iter()
            .enumerate()
        {
            assert_eq!(
                resolve_menu_entry(MenuKind::AbilitiesGroups, label, false, false),
                MenuDispatch::OpenSubmenu(MenuKind::AbilitiesGroup(idx as u8)),
                "{label} should open group {idx}",
            );
        }
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
    fn every_menu_action_takes_the_sub_target_confirm() {
        use kuluu_render::hud::menu::DynamicMenuAction as A;
        use kuluu_render::input_mode::SubTargetAction as S;
        // Boost (39) and Mighty Strikes (16) are validTarget SELF and still prompt.
        assert_eq!(
            sub_target_action_for(A::JobAbility { ability_id: 39 }),
            Some(S::Ability(39))
        );
        assert_eq!(
            sub_target_action_for(A::JobAbility { ability_id: 16 }),
            Some(S::Ability(16))
        );
        assert_eq!(
            sub_target_action_for(A::JobAbility { ability_id: 35 }),
            Some(S::Ability(35))
        );
        assert_eq!(
            sub_target_action_for(A::CastSpell { spell_id: 1 }),
            Some(S::Spell(1))
        );
        assert_eq!(sub_target_action_for(A::Dismount), None);
    }

    /// `conf13wi` rows answer their jump-table triples immediately: rows 1-3
    /// share store 0xC9 (values 0/1/2), rows 4-6 store 0xCE (1/2/0); marking
    /// follows the next row build (vault main_menu.md §59 addendum 2; bead
    /// kuluu-o9qt7.20.1).
    #[test]
    fn conf13wi_triples_write_the_option_store() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows, DynamicMenu};
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf13wi".to_string(),
            (0..6u32).map(|i| (600 + i, format!("c13_{i}"))).collect(),
        );
        let mut dynamic = DynamicMenu {
            rows: config_window_rows(
                "conf13wi",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..DynamicMenu::default()
        };
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigPage(11));
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        for (cursor, store_id, value) in [
            (0usize, 0xC9u32, 0i32),
            (1, 0xC9, 1),
            (2, 0xC9, 2),
            (3, 0xCE, 1),
            (4, 0xCE, 2),
            (5, 0xCE, 0),
        ] {
            stack.current_mut().unwrap().cursor = cursor;
            confirm_row_dynamic(
                &mut stack,
                &mut scene_state,
                &cmd_tx,
                &mut dynamic,
                &mut bag,
            );
            assert_eq!(dynamic.config_option_values.get(&store_id), Some(&value));
        }
        let rows = config_window_rows(
            "conf13wi",
            &table,
            &dynamic.config_option_values,
            None,
            &std::collections::BTreeMap::new(),
        );
        // store 0xCE currently holds 0 (row 6) - only that row wears the mark.
        assert_eq!(rows[5].label, "\u{bb} c13_5");
        assert!(!rows[4].label.starts_with('\u{bb}'));
    }

    /// `conf12wi`'s per-row sete/setne values (vault main_menu.md §59 addendum 2,
    /// corrected by addendum 5; bead kuluu-o9qt7.20.1): ordinary pairs write whole
    /// option dwords, rows 7/8 and 13/14 set single bits of the flag-mask option
    /// CONFIG_OPTION_MISC_FLAGS through helper FFXiMain.dll retail-2026-09 RVA
    /// 0x1938F0 — and each pair writes its own polarity, not a uniform off/on order.
    #[test]
    fn conf12wi_pairs_write_the_store_and_the_flag_mask_bits() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows, DynamicMenu};
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf12wi".to_string(),
            (0..14u32).map(|i| (800 + i, format!("c12_{i}"))).collect(),
        );
        let mut dynamic = DynamicMenu {
            rows: config_window_rows(
                "conf12wi",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..DynamicMenu::default()
        };
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigPage(10));
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        for cursor in [0usize, 1] {
            stack.current_mut().unwrap().cursor = cursor;
            confirm_row_dynamic(
                &mut stack,
                &mut scene_state,
                &cmd_tx,
                &mut dynamic,
                &mut bag,
            );
        }
        // The BE pair's on-member is select id 2 (row index 1).
        assert_eq!(dynamic.config_option_values.get(&0xBEu32), Some(&1i32));
        // Rows 7/8 (index 6/7) drive bit 7 of the flag mask — off then on.
        for cursor in [6usize, 7] {
            stack.current_mut().unwrap().cursor = cursor;
            confirm_row_dynamic(
                &mut stack,
                &mut scene_state,
                &cmd_tx,
                &mut dynamic,
                &mut bag,
            );
        }
        let mask = *dynamic
            .config_option_values
            .get(&kuluu_render::hud::menu::CONFIG_OPTION_MISC_FLAGS)
            .expect("the bit pair wrote the flag mask");
        assert_eq!(((mask as u32) >> 7) & 1, 1, "bit 7 ends set");
        // The helper's immediates are bits, never option ids (§59 addendum 5).
        assert!(!dynamic.config_option_values.contains_key(&7u32));
        // Rows 9/10 (index 8/9) write the primary store reversed: row 9 is on.
        stack.current_mut().unwrap().cursor = 8;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(dynamic.config_option_values.get(&0xC2u32), Some(&1i32));
    }

    /// One select on an `fxfilter` row flips only its own descriptor bit of option
    /// 0x92 — class C's whole-dword store .text FFXiMain.dll retail-2026-09 RVA
    /// 0x198390 (vault main_menu.md §59 addendum 14; bead kuluu-o9qt7.20.1.3). The
    /// page stays open, silent like every other config write: LSB accepts no such
    /// packet (`vendor/server/src/map/packets/c2s/0x0db_config_language.cpp`).
    #[test]
    fn fxfilter_rows_toggle_one_bit_and_keep_the_page_open() {
        use kuluu_render::hud::menu::{config_window_rows, ConfigDatRows, DynamicMenu};
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "fxfilter".to_string(),
            (0..18u32).map(|i| (153 + i, format!("fx{i}"))).collect(),
        );
        let mut dynamic = DynamicMenu {
            rows: config_window_rows(
                "fxfilter",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..DynamicMenu::default()
        };
        let mut stack = MenuStack::root();
        // fxfilter is hub page index 6 of RETAIL_CONFIG_PAGE_WINDOWS.
        stack.push(MenuKind::RetailConfigPage(6));
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();
        stack.current_mut().unwrap().cursor = 2;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&kuluu_render::hud::menu::CONFIG_OPTION_FILTER_CONSOLE_FLAGS),
            Some(&(1 << 2))
        );
        stack.current_mut().unwrap().cursor = 5;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&kuluu_render::hud::menu::CONFIG_OPTION_FILTER_CONSOLE_FLAGS),
            Some(&((1 << 2) | (1 << 5)))
        );
        // Toggling the first row again clears only its own bit.
        stack.current_mut().unwrap().cursor = 2;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&kuluu_render::hud::menu::CONFIG_OPTION_FILTER_CONSOLE_FLAGS),
            Some(&(1 << 5))
        );
        // The window never closed itself — retail's store method returns to the drawer.
        assert!(matches!(
            stack.current().unwrap().kind,
            MenuKind::RetailConfigPage(6)
        ));
    }

    /// A `cfilter` page select rides class B's shared select method onto the five persisted
    /// dwords — here through the seam on hub page index 1: a group row stores its level, a plain
    /// row toggles its pair-lane bit, and an interlock row's second select swaps L for C
    /// (.text FFXiMain.dll retail-2026-09 RVA 0x198080; vault main_menu.md §59 addendum 15; bead
    /// kuluu-o9qt7.20.1.3). Silent, and the page stays open like every other config write.
    #[test]
    fn cfilter_page_rows_apply_the_shared_method_and_keep_the_page_open() {
        use kuluu_render::hud::menu::{
            config_window_rows, ConfigDatRows, DynamicMenu, CFILTER_ROWS,
        };
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "cfilter".to_string(),
            CFILTER_ROWS
                .iter()
                .enumerate()
                .map(|(idx, row)| (row.caption_id, format!("cf{idx}")))
                .collect(),
        );
        let mut dynamic = DynamicMenu {
            rows: config_window_rows(
                "cfilter",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..DynamicMenu::default()
        };
        let mut stack = MenuStack::root();
        // cfilter is hub page index 1 of RETAIL_CONFIG_PAGE_WINDOWS.
        stack.push(MenuKind::RetailConfigPage(1));
        let mut scene_state = SceneState::default();
        let (cmd_tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();

        // Group row idx 52 stores level 1 (.text FFXiMain.dll retail-2026-09 RVA 0x1980AD).
        stack.current_mut().unwrap().cursor = 52;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&kuluu_render::hud::menu::CONFIG_OPTION_CFILTER_GROUP_LEVEL),
            Some(&1)
        );

        // Plain row idx 11 (kind 0x04) toggles its own lane bit, nothing else (.text
        // FFXiMain.dll retail-2026-09 RVA 0x1981EB).
        stack.current_mut().unwrap().cursor = 11;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&kuluu_render::hud::menu::CONFIG_OPTION_CFILTER_LANE_A),
            Some(&(1 << 4))
        );

        // Interlock row idx 5: first select sets L; the second clears it and forces C (.text
        // FFXiMain.dll retail-2026-09 RVA 0x198157–0x1981E8).
        stack.current_mut().unwrap().cursor = 5;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&kuluu_render::hud::menu::CONFIG_OPTION_CFILTER_LANE_B),
            Some(&(1 << (0x31 - 0x1e)))
        );
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&kuluu_render::hud::menu::CONFIG_OPTION_CFILTER_LANE_B),
            Some(&0)
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&kuluu_render::hud::menu::CONFIG_OPTION_CFILTER_CONSOLE),
            Some(&(1 << 9))
        );
        // The window never closed itself, and nothing reached the wire — LSB carries no packet
        // for these stores (vendor/server/src/map/packets/c2s/0x0db_config_language.cpp).
        assert!(matches!(
            stack.current().unwrap().kind,
            MenuKind::RetailConfigPage(1)
        ));
    }

    /// `conf12wi` rows 3/4 write the store-0xBF pair and, past helper FFXiMain.dll
    /// retail-2026-09 RVA 0x1A8030's gates, announce it — here through the seam on
    /// hub page index 10: enabling from no known stored value announces mode 4; a
    /// reselect inside the guard window (.text 0x1A7D05) stays silent while the
    /// store write stands; and Monstrosity bails before the wire (.text 0x1A8036)
    /// without rolling that write back (vault main_menu.md §59 addendum 16; bead
    /// kuluu-o9qt7.20.1.4).
    #[test]
    fn conf12wi_bf_pair_rows_write_the_store_and_announce_modes() {
        use kuluu_render::hud::menu::{
            config_window_rows, ConfigDatRows, DynamicMenu, CONFIG_OPTION_CONF_BF_PAIR,
            CONF_BF_MODE_DISABLE, CONF_BF_MODE_ENABLE,
        };
        let mut table = ConfigDatRows {
            loaded: true,
            ..Default::default()
        };
        table.by_root.insert(
            "conf12wi".to_string(),
            (0..14u32).map(|i| (900 + i, format!("bf{i}"))).collect(),
        );
        let mut dynamic = DynamicMenu {
            rows: config_window_rows(
                "conf12wi",
                &table,
                &std::collections::BTreeMap::new(),
                None,
                &std::collections::BTreeMap::new(),
            ),
            ..DynamicMenu::default()
        };
        let mut stack = MenuStack::root();
        stack.push(MenuKind::RetailConfigPage(10));
        let mut scene_state = SceneState::default();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(8);
        let mut bag = kuluu_render::hud::item_screen::ItemScreenContainer::default();

        // Row 4 (the id-4 member writing 1, .text FFXiMain.dll retail-2026-09 RVA
        // 0x19BA0E) enables from no known stored value → mode 4 (.text 0x19BA43).
        stack.current_mut().unwrap().cursor = 3;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&CONFIG_OPTION_CONF_BF_PAIR),
            Some(&1)
        );
        match cmd_rx.try_recv() {
            Ok(AgentCommand::AnnounceConfigBfPair { mode }) => {
                assert_eq!(mode, CONF_BF_MODE_ENABLE)
            }
            other => panic!("expected the enable announce, got {other:?}"),
        }

        // The off member (row 3 writes 0) right after: the write lands but the
        // guard (.text FFXiMain.dll retail-2026-09 RVA 0x1A7D05 `cmp eax, 0x2328;
        // seta`) suppresses inside its window — nothing new reaches the channel.
        stack.current_mut().unwrap().cursor = 2;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&CONFIG_OPTION_CONF_BF_PAIR),
            Some(&0)
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "the guard window must swallow the re-send"
        );

        // Monstrosity: the helper bails before touching the wire (.text FFXiMain.dll
        // retail-2026-09 RVA 0x1A8036) yet never rolls back the primary write.
        scene_state.snapshot.monstrosity = Some(kuluu_snapshot::MonstrosityInfo {
            species: 1,
            equipped_instincts: [0; 12],
        });
        stack.current_mut().unwrap().cursor = 3;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        assert_eq!(
            dynamic
                .config_option_values
                .get(&CONFIG_OPTION_CONF_BF_PAIR),
            Some(&1)
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "Monstrosity must stay off the wire"
        );

        // A forced-past stamp un-suppresses the next transition, whose off member is
        // helper(0) at .text FFXiMain.dll retail-2026-09 RVA 0x19BA5D.
        dynamic.conf_bf_stamp = Some(
            std::time::Instant::now()
                - std::time::Duration::from_millis(
                    kuluu_render::hud::menu::CONF_BF_ANNOUNCE_WINDOW_MS + 1,
                ),
        );
        stack.current_mut().unwrap().cursor = 2;
        scene_state.snapshot.monstrosity = None;
        confirm_row_dynamic(
            &mut stack,
            &mut scene_state,
            &cmd_tx,
            &mut dynamic,
            &mut bag,
        );
        match cmd_rx.try_recv() {
            Ok(AgentCommand::AnnounceConfigBfPair { mode }) => {
                assert_eq!(mode, CONF_BF_MODE_DISABLE)
            }
            other => panic!("expected the disable announce, got {other:?}"),
        }

        assert!(matches!(
            stack.current().unwrap().kind,
            MenuKind::RetailConfigPage(10)
        ));
    }
}

#[cfg(test)]
mod unity_chat_send_tests {
    use super::*;
    use kuluu_render::hud::menu::DynamicMenu;
    use kuluu_render::hud::unity_fp::{UnityChatToggle, UNITY_CHAT_OPTION};

    /// Zone-in sends the stored Unity chat mode once per zone; the open chat panel's change goes
    /// out on the next retail frame.
    #[test]
    fn zone_in_sends_the_stored_mode_and_the_panel_sends_its_change() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut app = App::new();
        app.insert_resource(crate::view_native::input::CommandTx(tx))
            .init_resource::<Time>()
            .init_resource::<SceneState>()
            .init_resource::<DynamicMenu>()
            .add_systems(Update, unity_chat_send_system);
        app.update();
        assert!(rx.try_recv().is_err(), "no zone yet");

        {
            let mut scene = app.world_mut().resource_mut::<SceneState>();
            scene.snapshot.zone_id = Some(230);
            scene.snapshot.zone_generation = 1;
        }
        app.world_mut()
            .resource_mut::<DynamicMenu>()
            .config_option_values
            .insert(UNITY_CHAT_OPTION, 1);
        app.update();
        assert_eq!(
            rx.try_recv(),
            Ok(AgentCommand::SetUnityChat {
                mode: ffxi_proto::map::unity_toggle_mode::ACTIVE
            })
        );
        app.update();
        assert!(rx.try_recv().is_err(), "once per zone-in");

        let mut chat = UnityChatToggle::open(1);
        chat.mode = ffxi_proto::map::unity_toggle_mode::INACTIVE;
        app.world_mut().resource_mut::<DynamicMenu>().unity_fp.chat = Some(chat);
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(
                1.0 / kuluu_render::scheduler_runtime::RETAIL_FPS,
            ));
        app.update();
        assert_eq!(
            rx.try_recv(),
            Ok(AgentCommand::SetUnityChat {
                mode: ffxi_proto::map::unity_toggle_mode::INACTIVE
            })
        );
    }

    /// The client option store lives for the session, so after a launch the Unity chat mode is
    /// unknown until the panel stores one: zone-in sends nothing rather than turning chat off.
    #[test]
    fn zone_in_sends_nothing_while_the_mode_is_unknown() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut app = App::new();
        app.insert_resource(crate::view_native::input::CommandTx(tx))
            .init_resource::<Time>()
            .init_resource::<SceneState>()
            .init_resource::<DynamicMenu>()
            .add_systems(Update, unity_chat_send_system);
        let zone_in = |app: &mut App, generation: u64| {
            let mut scene = app.world_mut().resource_mut::<SceneState>();
            scene.snapshot.zone_id = Some(230);
            scene.snapshot.zone_generation = generation;
        };
        zone_in(&mut app, 1);
        app.update();
        assert!(rx.try_recv().is_err(), "nothing stored, nothing sent");

        app.world_mut()
            .resource_mut::<DynamicMenu>()
            .config_option_values
            .insert(
                UNITY_CHAT_OPTION,
                i32::from(ffxi_proto::map::unity_toggle_mode::INACTIVE),
            );
        zone_in(&mut app, 2);
        app.update();
        assert_eq!(
            rx.try_recv(),
            Ok(AgentCommand::SetUnityChat {
                mode: ffxi_proto::map::unity_toggle_mode::INACTIVE
            }),
            "a stored mode goes out at the next zone-in"
        );
    }
}
