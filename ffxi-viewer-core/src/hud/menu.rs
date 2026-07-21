use bevy::prelude::*;

use crate::graphics_settings::{GraphicsSettings, GRAPHICS_FIELDS};
use crate::hud::style::{self, theme};
use crate::input_mode::{InputMode, MenuKind, MenuStack};

pub const ROOT_LOG_OUT: &str = "Log Out";
pub const ROOT_SHUT_DOWN: &str = "Shut Down";
// Label and position are provisional pending a retail main-menu capture
// (bead kuluu-y5hq retail_unknowns).
pub const ROOT_CURRENT_TIME: &str = "Current Time";

// "Communication" position is provisional pending a retail main-menu capture
// (bead kuluu-d4u retail_unknowns).
pub const ROOT_COMMUNICATION: &str = "Communication";
pub const COMM_EMOTE_LIST: &str = "Emote List";

// Retail's Command menu is a single pane with two pages, flipped with "-" (or
// the Left/Right arrows). ROOT_ENTRIES is page 1 followed by page 2; the page a
// cursor is on is derived from its index (root_page_bounds), so no separate page
// state is needed. Order mirrors retail HorizonXI for the commands we implement
// (Synthesis/Trade/Linkshell/Region Info/Missions/Quests/View House/Bazaar/Help
// Desk are dropped; Graphics/Debug are ours, grouped with the page-2 config-like
// commands).
const ROOT_ENTRIES: &[&str] = &[
    // Page 1
    "Status",
    "Equipment",
    "Magic",
    "Items",
    "Abilities",
    "Party",
    "Search",
    "Map",
    // Page 2
    "Key Items",
    "Macros",
    "Config",
    ROOT_CURRENT_TIME,
    ROOT_COMMUNICATION,
    "Graphics",
    "Debug",
    ROOT_SHUT_DOWN,
    ROOT_LOG_OUT,
];

const ROOT_PAGE_1_LEN: usize = 8;

/// The half-open `[start, end)` range of `ROOT_ENTRIES` the page containing
/// `cursor` occupies. The Command menu shows one page at a time; "-" and the
/// Left/Right arrows flip between them, Up/Down wrap within the current page.
pub fn root_page_bounds(cursor: usize) -> (usize, usize) {
    if cursor < ROOT_PAGE_1_LEN {
        (0, ROOT_PAGE_1_LEN)
    } else {
        (ROOT_PAGE_1_LEN, ROOT_ENTRIES.len())
    }
}

/// The cursor index that flips to the other Root page, landing on its first row.
pub fn root_other_page_cursor(cursor: usize) -> usize {
    if cursor < ROOT_PAGE_1_LEN {
        ROOT_PAGE_1_LEN
    } else {
        0
    }
}

/// The Root command list, for callers that enumerate the top-level entries
/// (e.g. the drill parity guard) without matching each label.
pub fn root_entries() -> &'static [&'static str] {
    ROOT_ENTRIES
}

const COMMUNICATION_ENTRIES: &[&str] = &[COMM_EMOTE_LIST];

const ITEMS_ENTRIES_STUB: &[&str] = &["(Items — Stage 3: pending inventory submenu)"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicMenuRow {
    pub label: String,

    pub action: DynamicMenuAction,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DynamicMenuAction {
    CastSpell {
        spell_id: u16,
    },

    JobAbility {
        ability_id: u16,
    },

    Weaponskill {
        skill_id: u16,
    },

    PetAbility {
        ability_id: u16,
    },

    /// Ranged attack (c2s action 0x10). Carries no id — only a target, picked
    /// through the same sub-target cursor as a weapon skill.
    RangedAttack,

    UseItem {
        container: u8,
        index: u8,
        item_no: u16,
    },

    /// c2s 0x029 ITEM_MOVE (full stack, server-picked destination slot).
    MoveItem {
        quantity: u32,
        from_container: u8,
        from_slot: u8,
        to_container: u8,
        item_no: u16,
    },

    /// Open the per-item context submenu for a slot.
    OpenItemAction {
        container: u8,
        index: u8,
        item_no: u16,
    },

    EquipItem {
        container: u8,
        container_index: u8,
        equip_slot: u8,
        item_no: u16,
    },

    /// Terminal Key Items row; selecting it echoes the name to chat, and
    /// cursoring over it marks the key item seen on menu close (c2s 0x064).
    KeyItem {
        id: u16,
    },

    /// Emote List row: dispatches the same AgentCommand::Emote as the slash
    /// command, at the current target.
    Emote {
        emote_id: u8,
    },
}

impl DynamicMenuAction {
    /// The item id a row acts on, for rows that carry one (item list, usable
    /// list, per-item context, equip). Lets the item window resolve icon +
    /// detail without matching each variant at every call site.
    pub fn item_no(&self) -> Option<u16> {
        match *self {
            DynamicMenuAction::UseItem { item_no, .. }
            | DynamicMenuAction::MoveItem { item_no, .. }
            | DynamicMenuAction::OpenItemAction { item_no, .. }
            | DynamicMenuAction::EquipItem { item_no, .. } => Some(item_no),
            _ => None,
        }
    }

    /// The source `(container, index)` of an item-bearing action, so per-slot
    /// state (charges, recast) resolves the exact instance rather than the
    /// first item of the same id.
    pub fn item_slot(&self) -> Option<(u8, u8)> {
        match *self {
            DynamicMenuAction::UseItem {
                container, index, ..
            }
            | DynamicMenuAction::OpenItemAction {
                container, index, ..
            } => Some((container, index)),
            DynamicMenuAction::EquipItem {
                container,
                container_index,
                ..
            } => Some((container, container_index)),
            _ => None,
        }
    }
}

/// One item row's label: the name, suffixed " xN" only for a real stack. Shared
/// by the inventory list and the usable-item list so partial stacks read the
/// same wherever an item appears.
pub fn item_qty_label(name: &str, quantity: u32) -> String {
    if quantity > 1 {
        format!("{name} x{quantity}")
    } else {
        name.to_string()
    }
}

/// Unseen ("new") key-item indicator suffix. Retail shows a yellow-bubble
/// glyph whose exact appearance is unverified (bead kuluu-h7x
/// retail_unknowns); a text marker stands in until a retail capture.
pub const KEY_ITEM_UNSEEN_SUFFIX: &str = " (new)";

pub fn key_item_row_label(id: u16, seen: bool) -> String {
    let name = ffxi_proto::key_item_names::lookup(id)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Key Item #{id}"));
    if seen {
        name
    } else {
        format!("{name}{KEY_ITEM_UNSEEN_SUFFIX}")
    }
}

#[derive(bevy::prelude::Resource, Debug, Clone, Default)]
pub struct DynamicMenu {
    pub rows: Vec<DynamicMenuRow>,
}

pub const DYNAMIC_VISIBLE_ROWS: usize = 22;

const EQUIPMENT_ENTRIES: &[&str] = &[
    "Main", "Sub", "Ranged", "Ammo", "Head", "Body", "Hands", "Legs", "Feet", "Neck", "Waist",
    "L.Ear", "R.Ear", "L.Ring", "R.Ring", "Back",
];

const STATUS_LABELS: &[&str] = &[
    "Profile",
    "Job Levels",
    "Master Levels",
    "Combat Skill",
    "Magic Skill",
    "Craft Skill",
    "Currencies",
    "Currencies 2",
    "Unity",
    "Play Time",
    "Merit Points",
    "Job Points",
];

const CONFIG_ENTRIES: &[&str] = &[
    "Standard",
    "Compact 1",
    "Compact 2",
    "Reset to defaults",
    "Show current bindings",
];

pub const DEBUG_PERF: &str = "Perf";
pub const DEBUG_TARGET_CYCLE: &str = "Target Cycle";
pub const DEBUG_MESH: &str = "Mesh Debug";
pub const DEBUG_NET_STATUS: &str = "Net Status";

const DEBUG_ENTRIES: &[&str] = &[DEBUG_PERF, DEBUG_TARGET_CYCLE, DEBUG_MESH, DEBUG_NET_STATUS];

const GRAPHICS_ENTRIES: &[&str] = &[
    "Preset",
    "Shadow Quality",
    "Shadow Cascades",
    "Shadow Distance",
    "Anti-Aliasing",
    "Texture Filtering",
    "Bloom",
    "Volumetric Fog",
    "Fog Quality",
    "View Distance",
    "VSync",
    "FOV",
    "Sky Style",
    "Water Style",
    "Dynamic Lights",
    "  Emitter Threshold",
    "  Emitter Intensity",
    "  Emitter Range",
    "  Flicker",
    "Shading",
    "Model Shadow Receiving",
    "Model Shadow Casting",
    "Depth of Field",
    "DoF Aperture",
    "Zone Lines",
    "Render Scale",
    "Reset to High",
];

pub const GRAPHICS_RESET_SLOT: usize = GRAPHICS_FIELDS.len();

const MAX_ENTRY_COUNT: usize = {
    let r = ROOT_ENTRIES.len();
    let c = CONFIG_ENTRIES.len();
    let g = GRAPHICS_ENTRIES.len();
    let e = EQUIPMENT_ENTRIES.len();
    let s = STATUS_LABELS.len();

    let d = DYNAMIC_VISIBLE_ROWS;
    let rc = if r >= c { r } else { c };
    let rcg = if rc >= g { rc } else { g };
    let rcge = if rcg >= e { rcg } else { e };
    let rcges = if rcge >= s { rcge } else { s };
    if rcges >= d {
        rcges
    } else {
        d
    }
};

pub fn is_dynamic(kind: MenuKind) -> bool {
    matches!(
        kind,
        MenuKind::Magic
            | MenuKind::Abilities
            | MenuKind::Items
            | MenuKind::KeyItems
            | MenuKind::UsableItems
            | MenuKind::ItemAction { .. }
            | MenuKind::EquipSlot(_)
            | MenuKind::EmoteList
    )
}

pub fn entry_count(kind: MenuKind, dynamic: &DynamicMenu) -> usize {
    if is_dynamic(kind) {
        dynamic.rows.len().max(1)
    } else {
        static_entries(kind).len()
    }
}

/// Rows to jump per Left/Right page in a vertical list menu — the visible-row
/// count of whichever panel renders `kind`, so one page-press scrolls a single
/// screenful (retail Left/Right paging). Items and the action-ring Usable list
/// ride the item_screen panel; everything else the generic panel.
pub fn list_page_rows(kind: MenuKind) -> usize {
    match kind {
        MenuKind::Items | MenuKind::UsableItems => crate::hud::item_screen::ITEM_LIST_ROWS,
        _ => DYNAMIC_VISIBLE_ROWS,
    }
}

/// New cursor after a Left/Right page press: jump `rows` toward `forward`
/// (Right/Down = true), clamped to the list — retail pages with no wrap, unlike
/// Up/Down which wrap one row at a time.
pub fn page_cursor(cursor: usize, entry_count: usize, rows: usize, forward: bool) -> usize {
    if forward {
        (cursor + rows).min(entry_count.saturating_sub(1))
    } else {
        cursor.saturating_sub(rows)
    }
}

pub fn entry_label(kind: MenuKind, idx: usize, dynamic: &DynamicMenu) -> &str {
    if is_dynamic(kind) {
        if dynamic.rows.is_empty() {
            return empty_dynamic_hint(kind);
        }
        return dynamic
            .rows
            .get(idx)
            .map(|r| r.label.as_str())
            .unwrap_or("<unknown>");
    }
    static_entries(kind)
        .get(idx)
        .copied()
        .unwrap_or("<unknown>")
}

pub fn entry_action(
    kind: MenuKind,
    idx: usize,
    dynamic: &DynamicMenu,
) -> Option<DynamicMenuAction> {
    if !is_dynamic(kind) {
        return None;
    }
    dynamic.rows.get(idx).map(|r| r.action)
}

fn empty_dynamic_hint(kind: MenuKind) -> &'static str {
    match kind {
        MenuKind::Magic => "(no spells learned yet)",
        MenuKind::Abilities => "(no abilities available — wrong job?)",
        MenuKind::Items => "(bag empty)",
        MenuKind::KeyItems => "(no key items)",
        MenuKind::UsableItems => "(no usable items)",
        MenuKind::ItemAction { .. } => "(item no longer in this bag)",
        MenuKind::EquipSlot(_) => "(no equippable items for this slot)",
        MenuKind::EmoteList => "(emote table unavailable)",
        _ => "(empty)",
    }
}

fn static_entries(kind: MenuKind) -> &'static [&'static str] {
    match kind {
        MenuKind::Root => ROOT_ENTRIES,
        MenuKind::Config => CONFIG_ENTRIES,
        MenuKind::Debug => DEBUG_ENTRIES,
        MenuKind::Graphics => GRAPHICS_ENTRIES,

        MenuKind::Magic => &["(Magic — data pending)"],
        MenuKind::Abilities => &["(Abilities — data pending)"],
        MenuKind::Items => ITEMS_ENTRIES_STUB,
        MenuKind::KeyItems => &[],
        MenuKind::UsableItems => &[],

        MenuKind::Equipment => EQUIPMENT_ENTRIES,

        MenuKind::Status => STATUS_LABELS,

        MenuKind::ItemAction { .. } => &[],
        MenuKind::EquipSlot(_) => &["(loading equippable items…)"],

        MenuKind::Communication => COMMUNICATION_ENTRIES,
        MenuKind::EmoteList => &[],
        // The Map screen renders its own bespoke panes; it has no generic list.
        MenuKind::Map => &[],
    }
}

pub fn menu_title(kind: MenuKind) -> &'static str {
    match kind {
        MenuKind::Root => "Commands",
        MenuKind::Config => "Config",
        MenuKind::Debug => "Debug",
        MenuKind::Graphics => "Graphics",
        MenuKind::Equipment => "Equipment",
        MenuKind::Magic => "Magic",
        MenuKind::Abilities => "Abilities",
        MenuKind::Items => "Items",
        MenuKind::KeyItems => "Key Items",
        MenuKind::UsableItems => "Items",
        MenuKind::ItemAction { .. } => "Item",
        MenuKind::Status => "Status",
        MenuKind::EquipSlot(_) => "Equip",
        MenuKind::Communication => "Communication",
        MenuKind::EmoteList => "Emote List",
        MenuKind::Map => "Map",
    }
}

/// The submenu a Root category drills into, and thus the level the right pane
/// previews while that category is highlighted at Root. Single source for the
/// Root → submenu mapping: `text_input::resolve_menu_entry` drills through this
/// so the client dispatch and the preview can't disagree (guarded by a test in
/// the client). `None` = a Root entry with no browsable submenu (Party/Search/
/// Macros fire commands or aren't built; Current Time/Log Out/Shut Down act
/// directly), so the right pane shows no preview.
pub fn root_child_kind(label: &str) -> Option<MenuKind> {
    Some(match label {
        "Magic" => MenuKind::Magic,
        "Abilities" => MenuKind::Abilities,
        "Items" => MenuKind::Items,
        "Key Items" => MenuKind::KeyItems,
        "Equipment" => MenuKind::Equipment,
        "Status" => MenuKind::Status,
        ROOT_COMMUNICATION => MenuKind::Communication,
        "Graphics" => MenuKind::Graphics,
        "Config" => MenuKind::Config,
        "Debug" => MenuKind::Debug,
        _ => return None,
    })
}

/// Kinds that render their own framed multi-panel screen (item_screen /
/// equipment_screen) instead of the generic two-pane list, so the generic pane
/// is suppressed and its pane-toggle does not apply while one is on top.
pub fn renders_bespoke_screen(kind: MenuKind) -> bool {
    matches!(
        kind,
        MenuKind::Equipment
            | MenuKind::EquipSlot(_)
            | MenuKind::Items
            | MenuKind::UsableItems
            | MenuKind::Map
    )
}

#[derive(Component)]
pub struct MainMenu;

#[derive(Component)]
pub struct MainMenuTitle;

#[derive(Component)]
pub struct MainMenuRow {
    pub slot: usize,
}

#[derive(Message, Debug, Clone, Copy)]
pub struct MenuRowActivated {
    pub slot: usize,
}

/// The single Command-menu column width; the Map screen's wide-scan column
/// reuses it so the pane widths can't drift.
pub const MENU_PANE_WIDTH: f32 = 220.0;

pub fn spawn_main_menu(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            MainMenu,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                right: Val::Px(8.0),
                display: Display::None,
                ..default()
            },
        ))
        .with_children(spawn_pane_column);
}

fn spawn_pane_column(parent: &mut ChildSpawnerCommands) {
    let (mut node, bg, border) = style::window_frame();
    node.width = Val::Px(MENU_PANE_WIDTH);
    parent.spawn((node, bg, border)).with_children(|c| {
        c.spawn((
            MainMenuTitle,
            Text::new(""),
            style::text_font(14.0),
            TextColor(theme::TITLE),
        ));

        for slot in 0..MAX_ENTRY_COUNT {
            c.spawn((
                MainMenuRow { slot },
                Button,
                Text::new(""),
                style::text_font(14.0),
                TextColor(theme::TEXT),
                BackgroundColor(Color::NONE),
            ));
        }
    });
}

pub fn refresh_dynamic_menu_rows(
    mode: Res<InputMode>,
    scene: Res<crate::snapshot::SceneState>,
    sort: Res<crate::hud::item_detail::SortOptions>,
    active_bag: Res<crate::hud::item_screen::ItemScreenContainer>,
    mut dynamic: ResMut<DynamicMenu>,
) {
    let active_kind = match &*mode {
        InputMode::Menu(stack) => stack.current().map(|l| l.kind),
        _ => None,
    };

    let Some(kind) = active_kind.filter(|k| is_dynamic(*k)) else {
        if !dynamic.rows.is_empty() {
            dynamic.rows.clear();
        }
        return;
    };
    let snap = &scene.snapshot;
    let rows: Vec<DynamicMenuRow> = match kind {
        MenuKind::Magic => snap
            .spells_known
            .iter()
            .filter_map(|&id| {
                ffxi_proto::spell_names::lookup(id).map(|name| DynamicMenuRow {
                    label: name.to_string(),
                    action: DynamicMenuAction::CastSpell { spell_id: id },
                })
            })
            .collect(),
        MenuKind::Abilities => {
            let mut out: Vec<DynamicMenuRow> = Vec::with_capacity(
                snap.job_abilities_known.len()
                    + snap.weaponskills_known.len()
                    + snap.pet_abilities_known.len(),
            );
            out.extend(snap.job_abilities_known.iter().filter_map(|&id| {
                ffxi_proto::ability_names::lookup(id).map(|name| DynamicMenuRow {
                    label: name.to_string(),
                    action: DynamicMenuAction::JobAbility { ability_id: id },
                })
            }));
            out.extend(snap.weaponskills_known.iter().filter_map(|&id| {
                ffxi_proto::ability_names::lookup(id).map(|name| DynamicMenuRow {
                    label: format!("{name} (WS)"),
                    action: DynamicMenuAction::Weaponskill { skill_id: id },
                })
            }));
            out.extend(snap.pet_abilities_known.iter().filter_map(|&id| {
                ffxi_proto::ability_names::lookup(id).map(|name| DynamicMenuRow {
                    label: format!("{name} (Pet)"),
                    action: DynamicMenuAction::PetAbility { ability_id: id },
                })
            }));
            out
        }
        MenuKind::Items => snap
            .container(active_bag.0)
            .map(|c| c.items.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter_map(|slot| {
                let name = ffxi_proto::item_names::lookup(slot.item_no)?;
                let label = item_qty_label(name, slot.quantity);
                Some(DynamicMenuRow {
                    label,
                    action: DynamicMenuAction::OpenItemAction {
                        container: slot.container,
                        index: slot.index,
                        item_no: slot.item_no,
                    },
                })
            })
            .collect(),
        MenuKind::KeyItems => key_item_rows(snap),
        MenuKind::EmoteList => emote_rows(snap),
        MenuKind::UsableItems => usable_item_rows(snap),
        MenuKind::ItemAction {
            container,
            index,
            item_no,
        } => item_action_rows(snap, container, index, item_no),
        MenuKind::EquipSlot(equip_slot) => {
            let (main_job, main_lv) = snap
                .self_char_id
                .and_then(|id| snap.party.iter().find(|m| m.id == id))
                .map(|m| (m.main_job, m.main_job_lv))
                .unwrap_or((0, 0));
            snap.inventory_main()
                .iter()
                .filter_map(|slot| {
                    let info = ffxi_proto::equip_info::lookup(slot.item_no)?;
                    if !ffxi_proto::equip_info::fits_slot(&info, equip_slot) {
                        return None;
                    }
                    if main_job != 0 && !ffxi_proto::equip_info::fits_job(&info, main_job) {
                        return None;
                    }
                    if main_lv != 0 && info.level > main_lv {
                        return None;
                    }
                    let name = ffxi_proto::item_names::lookup(slot.item_no)?;
                    let label = if info.level > 0 {
                        format!("{name} (Lv{})", info.level)
                    } else {
                        name.to_string()
                    };
                    Some(DynamicMenuRow {
                        label,
                        action: DynamicMenuAction::EquipItem {
                            container: slot.container,
                            container_index: slot.index,
                            equip_slot,
                            item_no: slot.item_no,
                        },
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    };

    let mut rows = rows;
    order_dynamic_rows(kind, sort.auto, &mut rows);
    if rows != dynamic.rows {
        dynamic.rows = rows;
    }
}

/// Order the freshly built rows for one dynamic menu. Retail's Items window
/// sorts by item id when "Auto" is on (grouping usable items, then weapons,
/// then armor by their DAT id ranges) and otherwise shows raw inventory-slot
/// order. The item context submenu keeps its built order; other dynamic menus
/// stay alphabetical.
fn order_dynamic_rows(kind: MenuKind, auto_sort: bool, rows: &mut [DynamicMenuRow]) {
    match kind {
        MenuKind::Items => {
            if auto_sort {
                rows.sort_by_key(item_row_sort_key);
            }
        }
        // Retail's Command Menu Items list is always id-sorted.
        MenuKind::UsableItems => rows.sort_by_key(item_row_sort_key),
        // Key items keep id order: ascending global id groups the 512-id
        // tables together (whether retail sections match the tables is an
        // open question, bead kuluu-h7x retail_unknowns). Emotes keep the
        // scraped-table (id) order.
        MenuKind::ItemAction { .. } | MenuKind::KeyItems | MenuKind::EmoteList => {}
        _ => rows.sort_by(|a, b| a.label.cmp(&b.label)),
    }
}

/// One row per owned key item, ascending global id, name from the scraped LSB
/// table, unseen ids suffixed with [`KEY_ITEM_UNSEEN_SUFFIX`].
pub fn key_item_rows(snap: &ffxi_viewer_wire::SceneSnapshot) -> Vec<DynamicMenuRow> {
    let mut ids: Vec<u16> = snap.key_items.clone();
    ids.sort_unstable();
    ids.into_iter()
        .map(|id| DynamicMenuRow {
            label: key_item_row_label(id, snap.key_items_seen.binary_search(&id).is_ok()),
            action: DynamicMenuAction::KeyItem { id },
        })
        .collect()
}

/// One row per canned emote from the scraped LSB table (id order): label +
/// "/command" column. HELM-only ids are server-initiated and skipped; the Job
/// row appears only when the 0x11A bitfield unlocks the current main job's
/// gesture (bit = job id - 1).
pub fn emote_rows(snap: &ffxi_viewer_wire::SceneSnapshot) -> Vec<DynamicMenuRow> {
    use ffxi_proto::map::emote;
    let main_job = snap
        .self_char_id
        .and_then(|id| snap.party.iter().find(|m| m.id == id))
        .map(|m| m.main_job)
        .unwrap_or(0);
    ffxi_proto::emote_names::EMOTES
        .iter()
        .filter(|&&(id, _)| !emote::HELM_ONLY.contains(&id))
        .filter(|&&(id, _)| {
            id != emote::JOB
                || snap
                    .emote_jobs
                    .is_some_and(|bits| ffxi_proto::decode::EmoteList::job_bit_set(bits, main_job))
        })
        .map(|&(id, name)| DynamicMenuRow {
            label: format!("{name} (/{})", emote_command_word(id)),
            action: DynamicMenuAction::Emote { emote_id: id },
        })
        .collect()
}

/// The slash-command word for an emote id — the scraped name lowercased,
/// except Job whose command is /jobemote.
pub fn emote_command_word(id: u8) -> String {
    if id == ffxi_proto::map::emote::JOB {
        return "jobemote".to_string();
    }
    ffxi_proto::emote_names::lookup(id)
        .map(str::to_lowercase)
        .unwrap_or_else(|| format!("emote{id}"))
}

fn item_row_sort_key(row: &DynamicMenuRow) -> u16 {
    match row.action {
        DynamicMenuAction::OpenItemAction { item_no, .. } => item_no,
        DynamicMenuAction::UseItem { item_no, .. } => item_no,
        _ => u16::MAX,
    }
}

/// LSB 0x037 item-use gate (vendor/server/src/map/packets/c2s — 0x037 item
/// use → CState checks + `item_usable`): an item can fire right now iff it
/// appears in `item_usable` and either
/// - it is a plain consumable (maxCharges == 0) sitting unlocked in
///   LOC_INVENTORY or LOC_TEMPITEMS, or
/// - it is charged equipment (maxCharges > 0) that is currently equipped
///   (equipping is what locks the slot; the equipped item ids are mirrored
///   in `SceneSnapshot::equipped`).
pub fn item_usable_now(
    snap: &ffxi_viewer_wire::SceneSnapshot,
    slot: &ffxi_viewer_wire::InventoryItem,
) -> bool {
    use ffxi_proto::map::container as c;

    let Some(info) = ffxi_proto::item_usable::lookup(slot.item_no) else {
        return false;
    };
    let is_equipment = ffxi_proto::equip_info::lookup(slot.item_no).is_some();
    if is_equipment {
        // Charged equipment: usable only while equipped. Equipment without
        // charges never fires from the menu even though it may sit in
        // item_usable (enchantment already consumed server-side).
        info.max_charges > 0
            && slot.locked
            && snap.equipped.contains(&Some(slot.item_no))
            && (slot.container == c::LOC_INVENTORY || c::is_wardrobe(slot.container))
    } else {
        // Consumables: LOC_INVENTORY / LOC_TEMPITEMS only, and never from a
        // locked (bazaar/linkshell-reserved) slot.
        (slot.container == c::LOC_INVENTORY || slot.container == c::LOC_TEMPITEMS) && !slot.locked
    }
}

/// Rows for the Command Menu "Items" submenu: every currently-usable item
/// across all containers, firing Use directly (kuluu-268h).
pub fn usable_item_rows(snap: &ffxi_viewer_wire::SceneSnapshot) -> Vec<DynamicMenuRow> {
    snap.containers
        .iter()
        .flat_map(|cont| cont.items.iter())
        .filter(|slot| item_usable_now(snap, slot))
        .filter_map(|slot| {
            let name = ffxi_proto::item_names::lookup(slot.item_no)?;
            let label = item_qty_label(name, slot.quantity);
            Some(DynamicMenuRow {
                label,
                action: DynamicMenuAction::UseItem {
                    container: slot.container,
                    index: slot.index,
                    item_no: slot.item_no,
                },
            })
        })
        .collect()
}

/// Whether the quick-menu "Items" entry should be enabled at all.
pub fn any_usable_item(snap: &ffxi_viewer_wire::SceneSnapshot) -> bool {
    snap.containers
        .iter()
        .flat_map(|cont| cont.items.iter())
        .any(|slot| item_usable_now(snap, slot))
}

/// Context rows for one slot, mirroring the LSB 0x029 move rules
/// (vendor/server/src/map/packets/c2s/0x029_item_move.cpp): Gil and Temporary
/// items never move, wardrobes only take equipment, and "Take Out" leads when
/// browsing a storage bag.
pub fn item_action_rows(
    snap: &ffxi_viewer_wire::SceneSnapshot,
    container: u8,
    index: u8,
    item_no: u16,
) -> Vec<DynamicMenuRow> {
    use ffxi_proto::map::container as c;

    let Some(slot) = snap
        .container(container)
        .and_then(|v| v.items.iter().find(|s| s.index == index))
        .filter(|s| s.item_no == item_no)
    else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    // Same predicate as the Command Menu Items list: only offer Use when the
    // LSB 0x037 gate would accept it (kuluu-268h).
    if item_usable_now(snap, slot) {
        rows.push(DynamicMenuRow {
            label: "Use".to_string(),
            action: DynamicMenuAction::UseItem {
                container,
                index,
                item_no,
            },
        });
    }

    // Locked = equipped / linkshell / bazaar-reserved: the server rejects the
    // move silently, so don't offer it.
    let movable =
        item_no != ffxi_proto::map::GIL_ITEM_NO && container != c::LOC_TEMPITEMS && !slot.locked;
    if movable {
        let equipable = ffxi_proto::equip_info::lookup(item_no).is_some();
        for dest in crate::hud::item_screen::accessible_containers(snap) {
            if dest == container || dest == c::LOC_TEMPITEMS || (c::is_wardrobe(dest) && !equipable)
            {
                continue;
            }
            let Some(name) = c::name(dest) else { continue };
            let label = if dest == c::LOC_INVENTORY {
                "Take Out".to_string()
            } else {
                format!("Put in {name}")
            };
            rows.push(DynamicMenuRow {
                label,
                action: DynamicMenuAction::MoveItem {
                    quantity: slot.quantity,
                    from_container: container,
                    from_slot: index,
                    to_container: dest,
                    item_no,
                },
            });
        }
    }
    rows
}

pub fn ability_group_rows(
    snap: &ffxi_viewer_wire::SceneSnapshot,
    group: crate::hud::action_model::AbilityGroup,
) -> Vec<DynamicMenuRow> {
    use crate::hud::action_model::AbilityGroup as G;
    let mut rows: Vec<DynamicMenuRow> = match group {
        G::JobAbilities => snap
            .job_abilities_known
            .iter()
            .filter_map(|&id| {
                ffxi_proto::ability_names::lookup(id).map(|name| DynamicMenuRow {
                    label: name.to_string(),
                    action: DynamicMenuAction::JobAbility { ability_id: id },
                })
            })
            .collect(),
        G::WeaponSkill => snap
            .weaponskills_known
            .iter()
            .filter_map(|&id| {
                ffxi_proto::ability_names::lookup(id).map(|name| DynamicMenuRow {
                    label: name.to_string(),
                    action: DynamicMenuAction::Weaponskill { skill_id: id },
                })
            })
            .collect(),
        G::PetCommand => snap
            .pet_abilities_known
            .iter()
            .filter_map(|&id| {
                ffxi_proto::ability_names::lookup(id).map(|name| DynamicMenuRow {
                    label: name.to_string(),
                    action: DynamicMenuAction::PetAbility { ability_id: id },
                })
            })
            .collect(),
        G::RangedAttack => vec![DynamicMenuRow {
            label: "Ranged Attack".to_string(),
            action: DynamicMenuAction::RangedAttack,
        }],
        G::Mount => Vec::new(),
    };
    rows.sort_by(|a, b| a.label.cmp(&b.label));
    rows
}

pub fn ability_group_empty_hint(group: crate::hud::action_model::AbilityGroup) -> &'static str {
    use crate::hud::action_model::AbilityGroup as G;
    match group {
        G::RangedAttack => "You cannot use that command here.",
        G::Mount => "No mounts available.",
        _ => "No abilities available.",
    }
}

/// What the single Command-menu column renders this frame: the current (top)
/// level of the stack.
struct PaneView {
    kind: MenuKind,
    cursor: usize,
}

fn current_view(stack: &MenuStack) -> Option<PaneView> {
    let level = stack.current()?;
    Some(PaneView {
        kind: level.kind,
        cursor: level.cursor,
    })
}

#[allow(clippy::type_complexity)]
pub fn update_main_menu(
    mode: Res<InputMode>,
    settings: Res<GraphicsSettings>,
    panels: Res<crate::hud::HudPanels>,
    net_status: Res<crate::hud::network_status::NetStatusVisible>,

    scene: Res<crate::snapshot::SceneState>,
    dynamic: Res<DynamicMenu>,
    mut menu_q: Query<&mut Node, (With<MainMenu>, Without<MainMenuRow>)>,
    mut row_q: Query<
        (
            &MainMenuRow,
            &mut Node,
            &mut Text,
            &mut TextColor,
            &mut BackgroundColor,
        ),
        Without<MainMenuTitle>,
    >,
    mut title_q: Query<(&MainMenuTitle, &mut Text), Without<MainMenuRow>>,
) {
    let Ok(mut node) = menu_q.single_mut() else {
        return;
    };

    let hide = |node: &mut Node| {
        if node.display != Display::None {
            node.display = Display::None;
        }
    };

    let stack = match &*mode {
        InputMode::Menu(stack) => stack,
        _ => {
            hide(&mut node);
            return;
        }
    };
    let Some(top_kind) = stack.current().map(|l| l.kind) else {
        hide(&mut node);
        return;
    };
    // The Equipment / Items screens render their own framed multi-panel layout;
    // suppress the generic two-pane while one is on top.
    if renders_bespoke_screen(top_kind) {
        hide(&mut node);
        return;
    }
    if node.display != Display::Flex {
        node.display = Display::Flex;
    }

    let view = current_view(stack);

    for (_, mut text) in title_q.iter_mut() {
        let want = view.as_ref().map(root_aware_title).unwrap_or_default();
        if **text != want {
            **text = want;
        }
    }

    let Some(view) = view else {
        for (_, mut row_node, _, _, _) in row_q.iter_mut() {
            hide(&mut row_node);
        }
        return;
    };

    let (total, viewport_start) = resolve_viewport(view.kind, view.cursor, &dynamic);
    let window = visible_window(view.kind, total);

    for (row, mut row_node, mut text, mut color, mut bg) in row_q.iter_mut() {
        let list_idx = viewport_start + row.slot;
        if row.slot >= window || list_idx >= total {
            hide(&mut row_node);
            continue;
        }
        if row_node.display != Display::Flex {
            row_node.display = Display::Flex;
        }

        let label_owned: String = if is_dynamic(view.kind) {
            entry_label(view.kind, list_idx, &dynamic).to_string()
        } else {
            static_entries(view.kind)
                .get(list_idx)
                .copied()
                .unwrap_or("<unknown>")
                .to_string()
        };
        let body = format_row_body(
            view.kind,
            list_idx,
            &label_owned,
            &settings,
            &panels,
            net_status.0,
            &scene.snapshot,
        );

        let is_cursor = view.cursor == list_idx;
        let (want, want_color, want_bg) = if is_cursor {
            (format!("> {body}"), theme::CURSOR, Color::NONE)
        } else {
            (format!("  {body}"), theme::TEXT, Color::NONE)
        };
        if **text != want {
            **text = want;
        }
        if color.0 != want_color {
            color.0 = want_color;
        }
        if bg.0 != want_bg {
            bg.0 = want_bg;
        }
    }
}

/// The pane title, with a `‹1/2›` page marker appended for the Root Command
/// menu so the two-page flip (retail's "-") is discoverable.
fn root_aware_title(view: &PaneView) -> String {
    let title = menu_title(view.kind);
    if view.kind == MenuKind::Root {
        let page = if view.cursor < ROOT_PAGE_1_LEN { 1 } else { 2 };
        format!("{title}  \u{2039}{page}/2\u{203a}")
    } else {
        title.to_string()
    }
}

fn visible_window(kind: MenuKind, total: usize) -> usize {
    if is_dynamic(kind) {
        DYNAMIC_VISIBLE_ROWS
    } else {
        total
    }
}

fn resolve_viewport(kind: MenuKind, cursor: usize, dynamic: &DynamicMenu) -> (usize, usize) {
    if kind == MenuKind::Root {
        // Show only the page the cursor is on: rows [start, end).
        let (start, end) = root_page_bounds(cursor);
        return (end, start);
    }
    if is_dynamic(kind) {
        let total = dynamic.rows.len().max(1);
        if total <= DYNAMIC_VISIBLE_ROWS {
            return (total, 0);
        }

        let half = DYNAMIC_VISIBLE_ROWS / 2;
        let max_start = total.saturating_sub(DYNAMIC_VISIBLE_ROWS);
        let start = cursor.saturating_sub(half).min(max_start);
        (total, start)
    } else {
        (static_entries(kind).len(), 0)
    }
}

pub fn menu_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    dynamic: Res<DynamicMenu>,
    rows: Query<(&Interaction, &MainMenuRow), Changed<Interaction>>,
) {
    let InputMode::Menu(stack) = &mut *mode else {
        return;
    };
    let Some(level_idx) = stack.levels.len().checked_sub(1) else {
        return;
    };
    let kind = stack.levels[level_idx].kind;
    for (interaction, row) in &rows {
        if !matches!(interaction, Interaction::Hovered | Interaction::Pressed) {
            continue;
        }
        let cursor = stack.levels[level_idx].cursor;
        let (total, viewport_start) = resolve_viewport(kind, cursor, &dynamic);
        let window = visible_window(kind, total);
        let list_idx = viewport_start + row.slot;
        if list_idx >= total || row.slot >= window {
            continue;
        }
        if stack.levels[level_idx].cursor != list_idx {
            stack.levels[level_idx].cursor = list_idx;
        }
    }
}

pub fn menu_mouse_click_system(
    mode: Res<InputMode>,
    dynamic: Res<DynamicMenu>,
    rows: Query<(&Interaction, &MainMenuRow), Changed<Interaction>>,
    mut out: MessageWriter<MenuRowActivated>,
) {
    let InputMode::Menu(stack) = &*mode else {
        return;
    };
    let Some(level) = stack.current() else {
        return;
    };
    let kind = level.kind;
    for (interaction, row) in &rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let (total, viewport_start) = resolve_viewport(kind, level.cursor, &dynamic);
        let window = visible_window(kind, total);
        let list_idx = viewport_start + row.slot;
        if list_idx < total && row.slot < window {
            out.write(MenuRowActivated { slot: list_idx });
        }
    }
}

fn format_row_body(
    kind: MenuKind,
    slot: usize,
    label: &str,
    settings: &GraphicsSettings,
    panels: &crate::hud::HudPanels,
    net_status_on: bool,
    snapshot: &ffxi_viewer_wire::SceneSnapshot,
) -> String {
    match kind {
        MenuKind::Graphics => match GRAPHICS_FIELDS.get(slot).copied() {
            Some(field) => format!(
                "{:<16}[{}]",
                format!("{}:", field.label()),
                settings.value_label(field)
            ),

            None => label.to_string(),
        },
        MenuKind::Debug => {
            let on = debug_panel_state(label, panels, net_status_on);
            format!("{label:<14}[{}]", if on { "on" } else { "off" })
        }
        MenuKind::Equipment => {
            let item_name = snapshot
                .equipped
                .get(slot)
                .copied()
                .flatten()
                .and_then(ffxi_proto::item_names::lookup)
                .unwrap_or("—");
            format!("{label:<7}: {item_name}")
        }
        _ => label.to_string(),
    }
}

pub fn debug_panel_state(label: &str, panels: &crate::hud::HudPanels, net_status_on: bool) -> bool {
    match label {
        DEBUG_PERF => panels.perf,
        DEBUG_TARGET_CYCLE => panels.target_cycle,
        DEBUG_MESH => panels.mesh_debug,
        DEBUG_NET_STATUS => net_status_on,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_pages_split_and_flip_by_cursor() {
        // Page 1 is the first ROOT_PAGE_1_LEN entries; page 2 the rest.
        let (s1, e1) = root_page_bounds(0);
        assert_eq!((s1, e1), (0, ROOT_PAGE_1_LEN));
        let (s2, e2) = root_page_bounds(ROOT_PAGE_1_LEN);
        assert_eq!((s2, e2), (ROOT_PAGE_1_LEN, ROOT_ENTRIES.len()));

        // "Map" lives on page 1, "Config" on page 2 (retail grouping).
        let map = ROOT_ENTRIES.iter().position(|e| *e == "Map").unwrap();
        let config = ROOT_ENTRIES.iter().position(|e| *e == "Config").unwrap();
        assert!(map < ROOT_PAGE_1_LEN && config >= ROOT_PAGE_1_LEN);

        // Flipping from either page lands on the other page's first row.
        assert_eq!(root_other_page_cursor(0), ROOT_PAGE_1_LEN);
        assert_eq!(root_other_page_cursor(ROOT_PAGE_1_LEN), 0);
    }

    fn root_menu_app(cursor: usize) -> App {
        use crate::hud::network_status::NetStatusVisible;
        use crate::snapshot::SceneState;

        let mut app = App::new();
        app.init_resource::<GraphicsSettings>()
            .init_resource::<crate::hud::HudPanels>()
            .init_resource::<NetStatusVisible>()
            .init_resource::<SceneState>()
            .init_resource::<DynamicMenu>();

        let mut stack = MenuStack::root();
        stack.current_mut().unwrap().cursor = cursor;
        app.insert_resource(InputMode::Menu(stack));

        app.add_systems(Startup, spawn_main_menu);
        app.add_systems(Update, update_main_menu);
        app.update();
        app
    }

    fn title_text(app: &mut App) -> String {
        let mut q = app.world_mut().query::<(&MainMenuTitle, &Text)>();
        q.iter(app.world())
            .map(|(_, text)| text.0.clone())
            .next()
            .expect("title exists")
    }

    fn visible_rows(app: &mut App) -> Vec<(usize, String)> {
        let mut q = app.world_mut().query::<(&MainMenuRow, &Text, &Node)>();
        let mut rows: Vec<(usize, String)> = q
            .iter(app.world())
            .filter(|(_, _, node)| node.display != Display::None)
            .map(|(row, text, _)| (row.slot, text.0.clone()))
            .collect();
        rows.sort_by_key(|(slot, _)| *slot);
        rows
    }

    #[test]
    fn single_pane_shows_only_the_cursors_root_page() {
        // Cursor on page 1 (Status at index 0): only page-1 entries render, and
        // the title carries the ‹1/2› marker.
        let mut app = root_menu_app(0);
        assert!(title_text(&mut app).contains("Commands"));
        assert!(title_text(&mut app).contains("1/2"));
        let rows = visible_rows(&mut app);
        assert_eq!(rows.len(), ROOT_PAGE_1_LEN, "only page 1 rows: {rows:?}");
        assert_eq!(rows[0].1, "> Status");
        assert!(rows.iter().any(|(_, t)| t.contains("Map")));
        assert!(
            !rows.iter().any(|(_, t)| t.contains("Config")),
            "page 2 entries stay hidden on page 1"
        );

        // Cursor on page 2 (Config): only page-2 entries render, marker flips.
        let config = ROOT_ENTRIES.iter().position(|e| *e == "Config").unwrap();
        let mut app = root_menu_app(config);
        assert!(title_text(&mut app).contains("2/2"));
        let rows = visible_rows(&mut app);
        assert_eq!(rows.len(), ROOT_ENTRIES.len() - ROOT_PAGE_1_LEN);
        assert!(rows.iter().any(|(_, t)| t.contains("Config")));
        assert!(!rows.iter().any(|(_, t)| t.contains("Status")));
    }

    #[test]
    fn list_page_rows_matches_owning_panel_viewport() {
        assert_eq!(
            list_page_rows(MenuKind::Items),
            crate::hud::item_screen::ITEM_LIST_ROWS
        );
        assert_eq!(
            list_page_rows(MenuKind::UsableItems),
            crate::hud::item_screen::ITEM_LIST_ROWS
        );
        assert_eq!(list_page_rows(MenuKind::Magic), DYNAMIC_VISIBLE_ROWS);
        assert_eq!(list_page_rows(MenuKind::KeyItems), DYNAMIC_VISIBLE_ROWS);
    }

    #[test]
    fn page_cursor_jumps_a_page_and_clamps_without_wrap() {
        // Forward jumps by `rows`, clamped to the last index (no wrap past end).
        assert_eq!(page_cursor(0, 30, 13, true), 13);
        assert_eq!(page_cursor(13, 30, 13, true), 26);
        assert_eq!(page_cursor(26, 30, 13, true), 29);
        assert_eq!(page_cursor(29, 30, 13, true), 29);
        // Backward jumps by `rows`, clamped to 0 (no wrap past start).
        assert_eq!(page_cursor(29, 30, 13, false), 16);
        assert_eq!(page_cursor(5, 30, 13, false), 0);
        assert_eq!(page_cursor(0, 30, 13, false), 0);
        // A page press moves more than one row — the whole point of the bug fix.
        assert!(page_cursor(0, 30, 13, true) > 1);
        // Empty list stays put.
        assert_eq!(page_cursor(0, 0, 13, true), 0);
    }

    #[test]
    fn item_qty_label_suffixes_only_real_stacks() {
        assert_eq!(item_qty_label("Bird Egg", 1), "Bird Egg");
        assert_eq!(item_qty_label("Bird Egg", 0), "Bird Egg");
        assert_eq!(item_qty_label("Bird Egg", 12), "Bird Egg x12");
    }

    #[test]
    fn action_item_no_covers_item_bearing_variants() {
        let use_it = DynamicMenuAction::UseItem {
            container: 0,
            index: 3,
            item_no: 4096,
        };
        let open = DynamicMenuAction::OpenItemAction {
            container: 0,
            index: 3,
            item_no: 4096,
        };
        assert_eq!(use_it.item_no(), Some(4096));
        assert_eq!(open.item_no(), Some(4096));
        assert_eq!(DynamicMenuAction::KeyItem { id: 5 }.item_no(), None);
        assert_eq!(DynamicMenuAction::CastSpell { spell_id: 1 }.item_no(), None);
    }

    fn mh_snapshot() -> ffxi_viewer_wire::SceneSnapshot {
        use ffxi_proto::map::container as c;
        let bag =
            |id: u8, capacity: u16, items: Vec<(u8, u16, u32)>| ffxi_viewer_wire::ContainerView {
                id,
                capacity,
                items: items
                    .into_iter()
                    .map(
                        |(index, item_no, quantity)| ffxi_viewer_wire::InventoryItem {
                            container: id,
                            index,
                            item_no,
                            quantity,
                            locked: false,
                            charges_remaining: None,
                            next_use_vana_ts: None,
                        },
                    )
                    .collect(),
            };
        ffxi_viewer_wire::SceneSnapshot {
            myroom: Some(ffxi_viewer_wire::MyRoom {
                model: 289,
                sub_map: 0,
            }),
            containers: vec![
                // Slot 0 carries Gil (retail keeps it in inventory slot 0).
                bag(
                    c::LOC_INVENTORY,
                    30,
                    vec![(0, ffxi_proto::map::GIL_ITEM_NO, 1000), (1, 4509, 12)],
                ),
                bag(c::LOC_MOGSAFE, 50, vec![(1, 13327, 1)]),
                bag(c::LOC_STORAGE, 8, vec![]),
                bag(c::LOC_TEMPITEMS, 20, vec![(0, 4212, 1)]),
                bag(c::LOC_WARDROBE, 8, vec![]),
            ],
            ..Default::default()
        }
    }

    /// Pins LSB 0x029 isValidMovement's ITEM_LOCKED rejection: locked slots
    /// (equipped / linkshell / bazaar-reserved) get no move rows. Since the
    /// 0x037 gate (kuluu-268h), a locked consumable also loses its Use row:
    /// the server rejects using bazaar/linkshell-reserved items.
    #[test]
    fn locked_slot_offers_no_moves() {
        use ffxi_proto::map::container as c;
        let mut snap = mh_snapshot();
        snap.containers[0].items[1].locked = true;
        let rows = item_action_rows(&snap, c::LOC_INVENTORY, 1, 4509);
        assert!(
            !rows
                .iter()
                .any(|r| matches!(r.action, DynamicMenuAction::MoveItem { .. })),
            "{rows:?}"
        );
        assert!(!rows.iter().any(|r| r.label == "Use"), "{rows:?}");
    }

    /// Pins the LSB validContainers Safe-2F gate (mhflag & 0x20,
    /// 0x029_item_move.cpp): capacity alone must not offer Mog Safe 2.
    #[test]
    fn safe2_requires_the_2f_flag() {
        use crate::hud::item_screen::container_accessible;
        use ffxi_proto::map::container as c;
        let mut snap = mh_snapshot();
        snap.containers.push(ffxi_viewer_wire::ContainerView {
            id: c::LOC_MOGSAFE2,
            capacity: 60,
            items: Vec::new(),
        });
        assert!(
            !container_accessible(&snap, c::LOC_MOGSAFE2),
            "flag unknown"
        );
        snap.mh_2f_unlocked = Some(false);
        assert!(!container_accessible(&snap, c::LOC_MOGSAFE2));
        snap.mh_2f_unlocked = Some(true);
        assert!(container_accessible(&snap, c::LOC_MOGSAFE2));
        assert!(
            container_accessible(&snap, c::LOC_MOGSAFE),
            "the 2F flag must not gate the 1F safe"
        );
    }

    #[test]
    fn storage_item_leads_with_take_out() {
        use ffxi_proto::map::container as c;
        // 13327 = an equipable ring in the retail id space; equip_info lookup
        // decides wardrobe eligibility, not this test.
        let rows = item_action_rows(&mh_snapshot(), c::LOC_MOGSAFE, 1, 13327);
        assert_eq!(rows[0].label, "Take Out");
        match rows[0].action {
            DynamicMenuAction::MoveItem {
                from_container,
                to_container,
                from_slot,
                quantity,
                ..
            } => {
                assert_eq!(from_container, c::LOC_MOGSAFE);
                assert_eq!(to_container, c::LOC_INVENTORY);
                assert_eq!(from_slot, 1);
                assert_eq!(quantity, 1);
            }
            _ => panic!("Take Out must be a MoveItem"),
        }
        assert!(
            !rows.iter().any(|r| r.label == "Use"),
            "storage bags cannot use items: {rows:?}"
        );
    }

    #[test]
    fn inventory_item_offers_use_and_put_in_bags() {
        use ffxi_proto::map::container as c;
        let rows = item_action_rows(&mh_snapshot(), c::LOC_INVENTORY, 1, 4509);
        assert_eq!(rows[0].label, "Use");
        assert!(rows.iter().any(|r| r.label == "Put in Mog Safe"));
        assert!(rows.iter().any(|r| r.label == "Put in Storage"));
        assert!(
            !rows.iter().any(|r| r.label.contains("Temporary")),
            "temp items bag is never a move destination: {rows:?}"
        );
        // 4509 (Distilled Water) is not equipment, so no wardrobe row.
        assert!(!rows.iter().any(|r| r.label.contains("Wardrobe")));
    }

    /// Gil and Temporary items never move (LSB 0x029 isValidMovement /
    /// validContainers).
    #[test]
    fn gil_and_temp_items_cannot_move() {
        use ffxi_proto::map::container as c;
        let snap = mh_snapshot();
        let gil = item_action_rows(&snap, c::LOC_INVENTORY, 0, ffxi_proto::map::GIL_ITEM_NO);
        assert!(
            !gil.iter()
                .any(|r| matches!(r.action, DynamicMenuAction::MoveItem { .. })),
            "{gil:?}"
        );
        let temp = item_action_rows(&snap, c::LOC_TEMPITEMS, 0, 4212);
        assert!(
            !temp
                .iter()
                .any(|r| matches!(r.action, DynamicMenuAction::MoveItem { .. })),
            "{temp:?}"
        );
        assert!(temp.iter().any(|r| r.label == "Use"), "{temp:?}");
    }

    #[test]
    fn stale_slot_yields_no_rows() {
        use ffxi_proto::map::container as c;
        let snap = mh_snapshot();
        assert!(item_action_rows(&snap, c::LOC_MOGSAFE, 5, 13327).is_empty());
        assert!(
            item_action_rows(&snap, c::LOC_MOGSAFE, 1, 999).is_empty(),
            "item id mismatch means the slot changed under the menu"
        );
    }

    fn war_party_member(id: u32) -> ffxi_viewer_wire::PartyMember {
        ffxi_viewer_wire::PartyMember {
            id,
            act_index: 0x100,
            name: Some("Kupo".into()),
            hp: 30,
            mp: 0,
            tp: 0,
            hp_pct: 100,
            mp_pct: 0,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 1,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: true,
            is_alliance_leader: false,
            in_mog_house: false,
        }
    }

    /// Pins the emote-list gating: HELM-only ids never appear (server-initiated,
    /// emote.h), and the Job row needs both a known main job and its 0x11A bit.
    #[test]
    fn emote_rows_skip_helm_and_gate_job_on_0x11a_bits() {
        use ffxi_proto::map::emote;
        let mut snap = ffxi_viewer_wire::SceneSnapshot::default();
        let rows = emote_rows(&snap);
        assert!(rows.iter().any(|r| r.label == "Wave (/wave)"));
        assert!(rows.iter().any(|r| r.label == "Aim (/aim)"));
        assert!(
            !rows.iter().any(|r| r.label.contains("Logging")),
            "HELM-only emotes are server-initiated"
        );
        let is_job = |r: &DynamicMenuRow| matches!(r.action, DynamicMenuAction::Emote { emote_id } if emote_id == emote::JOB);
        assert!(
            !rows.iter().any(is_job),
            "Job row hidden until 0x11A unlocks it"
        );

        snap.self_char_id = Some(1);
        snap.party = vec![war_party_member(1)];
        snap.emote_jobs = Some(0);
        assert!(!emote_rows(&snap).iter().any(is_job), "WAR bit not set");
        snap.emote_jobs = Some(1 << 0);
        assert!(
            emote_rows(&snap).iter().any(is_job),
            "WAR main + WAR gesture bit shows the Job row"
        );

        snap.party[0].main_job = 33;
        snap.emote_jobs = Some(u32::MAX);
        assert!(
            !emote_rows(&snap).iter().any(is_job),
            "wire-supplied main job past the u32 bit width reads locked (no shift panic)"
        );
    }

    #[test]
    fn emote_command_words_lowercase_names_except_job() {
        use ffxi_proto::map::emote;
        assert_eq!(emote_command_word(8), "wave");
        assert_eq!(emote_command_word(65), "dance1");
        assert_eq!(emote_command_word(emote::JOB), "jobemote");
    }

    #[test]
    fn status_labels_match_entries() {
        use crate::hud::status_panel::STATUS_ENTRIES;
        assert_eq!(STATUS_LABELS.len(), STATUS_ENTRIES.len());
        for (i, entry) in STATUS_ENTRIES.iter().enumerate() {
            assert_eq!(STATUS_LABELS[i], entry.label, "Status row {i} label drift");
        }
    }

    #[test]
    fn static_menus_fit_pool_and_show_all_rows() {
        for kind in [
            MenuKind::Root,
            MenuKind::Config,
            MenuKind::Debug,
            MenuKind::Graphics,
            MenuKind::Equipment,
            MenuKind::Status,
            MenuKind::Communication,
        ] {
            let total = static_entries(kind).len();
            assert!(
                total <= MAX_ENTRY_COUNT,
                "{kind:?} has {total} rows, exceeds row pool {MAX_ENTRY_COUNT}"
            );
            assert_eq!(
                visible_window(kind, total),
                total,
                "{kind:?} is static and must render every row"
            );
        }
    }

    #[test]
    fn current_time_appears_exactly_once_in_root() {
        assert_eq!(
            ROOT_ENTRIES
                .iter()
                .filter(|l| **l == ROOT_CURRENT_TIME)
                .count(),
            1
        );
    }

    #[test]
    fn debug_rows_map_to_distinct_panel_state() {
        let panels = crate::hud::HudPanels {
            perf: true,
            target_cycle: false,
            mesh_debug: true,
        };
        assert!(debug_panel_state(DEBUG_PERF, &panels, false));
        assert!(!debug_panel_state(DEBUG_TARGET_CYCLE, &panels, false));
        assert!(debug_panel_state(DEBUG_MESH, &panels, false));
        assert!(debug_panel_state(DEBUG_NET_STATUS, &panels, true));
        assert!(!debug_panel_state(DEBUG_NET_STATUS, &panels, false));

        for label in DEBUG_ENTRIES {
            assert_eq!(
                static_entries(MenuKind::Debug)
                    .iter()
                    .filter(|e| *e == label)
                    .count(),
                1,
                "Debug row {label:?} must appear exactly once"
            );
        }
    }

    /// Two rows whose label order (Apple, Zeta) disagrees with their item-id
    /// order (50, 10), so each ordering branch is distinguishable.
    fn conflicting_item_rows() -> Vec<DynamicMenuRow> {
        let item_row = |item_no: u16| DynamicMenuAction::OpenItemAction {
            container: 0,
            index: 0,
            item_no,
        };
        vec![
            DynamicMenuRow {
                label: "Apple".to_string(),
                action: item_row(50),
            },
            DynamicMenuRow {
                label: "Zeta".to_string(),
                action: item_row(10),
            },
        ]
    }

    fn labels(rows: &[DynamicMenuRow]) -> Vec<&str> {
        rows.iter().map(|r| r.label.as_str()).collect()
    }

    #[test]
    fn key_item_rows_sorted_by_id_with_unseen_suffix() {
        // Key items 1/8 = Zeruhn Report / Airship Pass
        // (vendor/server/scripts/enum/key_item.lua).
        let snap = ffxi_viewer_wire::SceneSnapshot {
            key_items: vec![8, 1],
            key_items_seen: vec![8],
            ..Default::default()
        };
        let rows = key_item_rows(&snap);
        assert_eq!(
            labels(&rows),
            [
                format!("Zeruhn Report{KEY_ITEM_UNSEEN_SUFFIX}").as_str(),
                "Airship Pass"
            ]
        );
        assert_eq!(rows[0].action, DynamicMenuAction::KeyItem { id: 1 });

        let mut rows = rows;
        order_dynamic_rows(MenuKind::KeyItems, true, &mut rows);
        assert_eq!(
            rows[0].action,
            DynamicMenuAction::KeyItem { id: 1 },
            "key items keep id order, never alphabetical"
        );
    }

    #[test]
    fn item_rows_sort_by_id_when_auto() {
        let mut rows = conflicting_item_rows();
        order_dynamic_rows(MenuKind::Items, true, &mut rows);
        assert_eq!(labels(&rows), ["Zeta", "Apple"]);
    }

    #[test]
    fn item_rows_keep_slot_order_when_manual() {
        let mut rows = conflicting_item_rows();
        order_dynamic_rows(MenuKind::Items, false, &mut rows);
        assert_eq!(labels(&rows), ["Apple", "Zeta"]);
    }

    #[test]
    fn item_action_rows_keep_built_order() {
        let mut rows = conflicting_item_rows();
        order_dynamic_rows(
            MenuKind::ItemAction {
                container: 0,
                index: 0,
                item_no: 50,
            },
            true,
            &mut rows,
        );
        assert_eq!(labels(&rows), ["Apple", "Zeta"]);
    }

    #[test]
    fn other_dynamic_menus_sort_alphabetically() {
        let mut rows = conflicting_item_rows();
        rows.swap(0, 1); // start Zeta-first so the sort has work to do
        order_dynamic_rows(MenuKind::Magic, true, &mut rows);
        assert_eq!(labels(&rows), ["Apple", "Zeta"]);
    }

    #[test]
    fn graphics_entries_match_field_labels() {
        assert_eq!(
            GRAPHICS_ENTRIES.len(),
            GRAPHICS_FIELDS.len() + 1,
            "expected one row per field + a trailing Reset row"
        );
        for (i, field) in GRAPHICS_FIELDS.iter().enumerate() {
            assert_eq!(
                GRAPHICS_ENTRIES[i],
                field.label(),
                "row {i} label drift: entry={:?}, field.label()={:?}",
                GRAPHICS_ENTRIES[i],
                field.label()
            );
        }
        assert_eq!(
            GRAPHICS_ENTRIES[GRAPHICS_RESET_SLOT], "Reset to High",
            "the slot past the last field must be the Reset action"
        );
    }

    /// One-slot snapshot builder for the LSB 0x037 usability gate tests
    /// (kuluu-268h). Item ids pinned against vendor/server/sql:
    /// 4112 = potion (consumable), 15840 = kupofried's ring (charged
    /// equipment), 13327 = silver earring (plain equipment, no activation).
    fn slot_snapshot(container: u8, item_no: u16, locked: bool) -> ffxi_viewer_wire::SceneSnapshot {
        ffxi_viewer_wire::SceneSnapshot {
            containers: vec![ffxi_viewer_wire::ContainerView {
                id: container,
                capacity: 30,
                items: vec![ffxi_viewer_wire::InventoryItem {
                    container,
                    index: 0,
                    item_no,
                    quantity: 1,
                    locked,
                    charges_remaining: None,
                    next_use_vana_ts: None,
                }],
            }],
            ..Default::default()
        }
    }

    fn only_slot(snap: &ffxi_viewer_wire::SceneSnapshot) -> &ffxi_viewer_wire::InventoryItem {
        &snap.containers[0].items[0]
    }

    /// Consumables fire from LOC_INVENTORY and LOC_TEMPITEMS only
    /// (0x037 → CState container check).
    #[test]
    fn consumable_usable_from_inventory_and_tempitems_only() {
        use ffxi_proto::map::container as c;
        for (container, expect) in [
            (c::LOC_INVENTORY, true),
            (c::LOC_TEMPITEMS, true),
            (c::LOC_MOGSAFE, false),
            (c::LOC_STORAGE, false),
            (c::LOC_WARDROBE, false),
        ] {
            let snap = slot_snapshot(container, 4112, false);
            assert_eq!(
                item_usable_now(&snap, only_slot(&snap)),
                expect,
                "potion in container {container}"
            );
        }
    }

    /// Locked (bazaar/linkshell-reserved) consumables are rejected by the
    /// server, so the menu must not offer them.
    #[test]
    fn locked_consumable_is_not_usable() {
        use ffxi_proto::map::container as c;
        let snap = slot_snapshot(c::LOC_INVENTORY, 4112, true);
        assert!(!item_usable_now(&snap, only_slot(&snap)));
    }

    /// Charged equipment (kupofried's ring) is usable only while equipped:
    /// locked slot + mirrored in SceneSnapshot::equipped.
    #[test]
    fn charged_equipment_usable_only_while_equipped() {
        use ffxi_proto::map::container as c;
        let mut snap = slot_snapshot(c::LOC_INVENTORY, 15840, true);
        snap.equipped[13] = Some(15840);
        assert!(item_usable_now(&snap, only_slot(&snap)));

        // Sitting unequipped in the bag: not usable.
        let unequipped = slot_snapshot(c::LOC_INVENTORY, 15840, false);
        assert!(!item_usable_now(&unequipped, only_slot(&unequipped)));

        // Equipped but stored via a non-wardrobe container: not usable.
        let mut stored = slot_snapshot(c::LOC_MOGSAFE, 15840, true);
        stored.equipped[13] = Some(15840);
        assert!(!item_usable_now(&stored, only_slot(&stored)));
    }

    /// Plain equipment (no item_usable entry) never appears, even equipped.
    #[test]
    fn plain_equipment_is_never_usable() {
        use ffxi_proto::map::container as c;
        let mut snap = slot_snapshot(c::LOC_INVENTORY, 13327, true);
        snap.equipped[11] = Some(13327);
        assert!(!item_usable_now(&snap, only_slot(&snap)));
    }

    /// The quick-menu Items rows fire Use directly and skip unusable slots;
    /// any_usable_item gates the entry itself.
    #[test]
    fn usable_item_rows_fire_use_directly() {
        use ffxi_proto::map::container as c;
        let snap = slot_snapshot(c::LOC_INVENTORY, 4112, false);
        let rows = usable_item_rows(&snap);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(matches!(
            rows[0].action,
            DynamicMenuAction::UseItem {
                container: c::LOC_INVENTORY,
                index: 0,
                item_no: 4112,
            }
        ));
        assert!(any_usable_item(&snap));

        let safe = slot_snapshot(c::LOC_MOGSAFE, 4112, false);
        assert!(usable_item_rows(&safe).is_empty());
        assert!(!any_usable_item(&safe));
    }
}
