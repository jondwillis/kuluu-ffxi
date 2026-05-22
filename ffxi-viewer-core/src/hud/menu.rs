//! Main menu (`-` to open) — vanilla-FFXI-styled vertical list anchored
//! to the right side of the screen.
//!
//! The menu is *mostly* a scaffold today: most root-level entries still
//! emit `[menu] Magic — not implemented` when selected. The exception is
//! `Config`, which pushes a [`MenuKind::Config`] submenu listing the
//! keybind presets — selecting one is equivalent to
//! `/keybinds preset <name>`. The Logout entry is wired to the real
//! `ReqLogout` packet.
//!
//! Look:
//!   - right-anchored, dark backdrop with a thin border,
//!   - `> Magic` style cursor on the selected row (cyan accent),
//!   - other rows in muted text.
//!
//! Visibility is driven by [`InputMode::Menu`]; otherwise `Display::None`.

use bevy::prelude::*;

use crate::graphics_settings::{GraphicsSettings, GRAPHICS_FIELDS};
use crate::hud::palette;
use crate::input_mode::{InputMode, MenuKind};

/// Fixed root-level entry labels. Order matches vanilla FFXI's main menu
/// roughly. `text_input::handle_menu_key` reads this list (via
/// [`entry_label`] / [`entry_count`]) so cursor bounds match what the
/// user actually sees.
const ROOT_ENTRIES: &[&str] = &[
    "Magic",
    "Abilities",
    "Items",
    "Equipment",
    "Status",
    "Party",
    "Search",
    "Macros",
    "Graphics",
    "Config",
    "Logout",
];

/// Placeholder rows for action submenus that haven't been wired to
/// real data yet. Each submenu replaces this slice with its real
/// source in a later stage. Stage 2 wires Magic + Abilities to the
/// dynamic-menu resource (see [`DynamicMenu`]); Items lands in Stage 3.
const ITEMS_ENTRIES_STUB: &[&str] = &["(Items — Stage 3: pending inventory submenu)"];

/// One row of a dynamic (data-driven) submenu — Magic, Abilities, etc.
/// Populated by `refresh_dynamic_menu_rows` each frame from the
/// `SceneSnapshot`'s `spells_known` / `job_abilities_known` /
/// `weaponskills_known` / `pet_abilities_known` mirrors. The row
/// carries both the *display* label and the *dispatch* action so the
/// keyboard handler can fire the right packet without re-deriving from
/// the label string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicMenuRow {
    /// Pre-formatted display text (e.g. "Cure" / "Berserk").
    pub label: String,
    /// What pressing Enter should dispatch.
    pub action: DynamicMenuAction,
}

/// Distinguishes what kind of action a dynamic row dispatches. The
/// resolver in `ffxi-client/src/view_native/text_input.rs` maps each
/// variant onto the matching `ActionKind` (the same wire path the
/// existing `/cast` / `/ja` / `/ws` slash commands already use).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DynamicMenuAction {
    /// Cast a magic spell. Maps to `ActionKind::CastMagic`.
    CastSpell { spell_id: u16 },
    /// Use a job ability. Maps to `ActionKind::JobAbility`.
    JobAbility { ability_id: u16 },
    /// Use a weapon skill. Maps to `ActionKind::Weaponskill`.
    Weaponskill { skill_id: u16 },
    /// Use a pet ability / blood pact. Currently dispatched as
    /// `ActionKind::JobAbility` (LSB sends pet commands via the same
    /// 0x1A action packet with the pet's ability id); refine if a
    /// future split is needed.
    PetAbility { ability_id: u16 },
    /// Use an inventory item. Dispatched as `AgentCommand::UseItem`
    /// (not `Action`) — items use packet 0x37 ITEM_USE, not 0x1A.
    /// Target defaults to self for consumables; the dispatcher
    /// substitutes the current target when present.
    UseItem {
        container: u8,
        index: u8,
        item_no: u16,
    },
}

/// Per-frame snapshot of the active dynamic submenu's rows + viewport
/// offset. Built by [`refresh_dynamic_menu_rows`] from the
/// `SceneSnapshot`; read by [`update_main_menu`] (for rendering) and
/// by the keyboard handler (for cursor clamping + Enter dispatch).
///
/// `viewport_start` is recomputed each frame from the cursor —
/// scrolling is purely a function of cursor position, so navigation
/// keys don't need a separate "scroll up/down" path.
#[derive(bevy::prelude::Resource, Debug, Clone, Default)]
pub struct DynamicMenu {
    /// All rows for the active dynamic submenu (empty if the active
    /// menu kind is not dynamic — Root / Config / Graphics / Equipment).
    pub rows: Vec<DynamicMenuRow>,
}

/// Maximum number of dynamic-menu rows visible at once. Above this,
/// the renderer windows the list and the cursor anchors to the
/// middle as the operator scrolls past the visible region.
pub const DYNAMIC_VISIBLE_ROWS: usize = 22;

/// Retail FFXI equipment slot names, ordered to match LSB's `SLOTTYPE`
/// enum (`vendor/server/src/map/enums/slot.h`): 0=Main, 1=Sub, ...,
/// 15=Back. Index parity is load-bearing — `update_main_menu` indexes
/// `SceneSnapshot.equipped[row.slot]` against the same i.
const EQUIPMENT_ENTRIES: &[&str] = &[
    "Main", "Sub", "Ranged", "Ammo", "Head", "Body", "Hands", "Legs", "Feet", "Neck", "Waist",
    "L.Ear", "R.Ear", "L.Ring", "R.Ring", "Back",
];

/// Config submenu entries. Order is "presets first, smallest delta from
/// retail names first; meta-entries last." The labels pass through to
/// `text_input::resolve_menu_entry` for `MenuKind::Config`, which maps
/// each one to a `KeybindUpdate`. Keep the labels stable — they're the
/// match-string surface for the dispatcher.
const CONFIG_ENTRIES: &[&str] = &[
    "Standard",
    "Compact 1",
    "Compact 2",
    "Reset to defaults",
    "Show current bindings",
];

/// Graphics submenu entries. The first N rows correspond 1:1 to
/// [`GRAPHICS_FIELDS`] — index `i` → cycle [`GRAPHICS_FIELDS[i]`]. The
/// final row is the "Reset to High" meta action.
///
/// Labels must mirror [`crate::graphics_settings::GraphicsField::label`];
/// a unit test in this module enforces parity.
const GRAPHICS_ENTRIES: &[&str] = &[
    "Preset",
    "Shadow Quality",
    "Shadow Cascades",
    "Shadow Distance",
    "Anti-Aliasing",
    "Bloom",
    "Volumetric Fog",
    "Fog Quality",
    "View Distance",
    "VSync",
    "FOV",
    "Reset to High",
];

/// Sentinel index into [`GRAPHICS_ENTRIES`] for the meta "Reset to High"
/// row. The dispatcher in `text_input::resolve_menu_entry` matches by
/// label, but the renderer uses this index to know when to skip the
/// `Field: [Value]` formatting and just print the action name.
pub const GRAPHICS_RESET_SLOT: usize = GRAPHICS_FIELDS.len();

/// Largest entry count across all menu kinds. Drives the spawn-time row
/// pool — we spawn this many rows once and toggle their visibility per
/// frame depending on the active menu kind, instead of spawning/despawning
/// when the menu changes screens.
const MAX_ENTRY_COUNT: usize = {
    let r = ROOT_ENTRIES.len();
    let c = CONFIG_ENTRIES.len();
    let g = GRAPHICS_ENTRIES.len();
    let e = EQUIPMENT_ENTRIES.len();
    // The dynamic menus (Magic / Abilities) get DYNAMIC_VISIBLE_ROWS
    // worth of viewport — beyond that the list scrolls, so the row
    // pool only needs the visible-window size.
    let d = DYNAMIC_VISIBLE_ROWS;
    let rc = if r >= c { r } else { c };
    let rcg = if rc >= g { rc } else { g };
    let rcge = if rcg >= e { rcg } else { e };
    if rcge >= d {
        rcge
    } else {
        d
    }
};

/// Returns true when the named submenu draws from [`DynamicMenu`]
/// instead of one of the static `*_ENTRIES` slices. The cursor
/// router uses this to decide whether to clamp against the
/// resource-driven count or the static-slice count.
pub fn is_dynamic(kind: MenuKind) -> bool {
    matches!(
        kind,
        MenuKind::Magic | MenuKind::Abilities | MenuKind::Items
    )
}

/// Number of entries on the named menu screen. For static menus this
/// is constant; for dynamic menus (Magic / Abilities) it reflects the
/// current row count in [`DynamicMenu`]. Falls back to 1 when the
/// dynamic resource is empty so the cursor still has a valid landing
/// row (the placeholder "no spells learned" hint).
pub fn entry_count(kind: MenuKind, dynamic: &DynamicMenu) -> usize {
    if is_dynamic(kind) {
        dynamic.rows.len().max(1)
    } else {
        static_entries(kind).len()
    }
}

/// Label for a given menu screen + cursor index. For dynamic menus the
/// label comes from `DynamicMenu.rows[idx].label`; out-of-range
/// returns `"<unknown>"` rather than panicking.
pub fn entry_label<'a>(kind: MenuKind, idx: usize, dynamic: &'a DynamicMenu) -> &'a str {
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
    static_entries(kind).get(idx).copied().unwrap_or("<unknown>")
}

/// Resolve a dynamic-menu row's dispatch action by cursor index.
/// Returns `None` when the submenu isn't dynamic, when the resource
/// is empty (the placeholder row has no action), or when the index
/// is out of range.
pub fn entry_action(kind: MenuKind, idx: usize, dynamic: &DynamicMenu) -> Option<DynamicMenuAction> {
    if !is_dynamic(kind) {
        return None;
    }
    dynamic.rows.get(idx).map(|r| r.action)
}

fn empty_dynamic_hint(kind: MenuKind) -> &'static str {
    match kind {
        MenuKind::Magic => "(no spells learned yet)",
        MenuKind::Abilities => "(no abilities available — wrong job?)",
        MenuKind::Items => "(inventory empty)",
        _ => "(empty)",
    }
}

fn static_entries(kind: MenuKind) -> &'static [&'static str] {
    match kind {
        MenuKind::Root => ROOT_ENTRIES,
        MenuKind::Config => CONFIG_ENTRIES,
        MenuKind::Graphics => GRAPHICS_ENTRIES,
        // Stage 2 routes Magic/Abilities through DynamicMenu; the
        // static slice path returns a one-row hint as a fallback when
        // a caller can't reach the resource.
        MenuKind::Magic => &["(Magic — data pending)"],
        MenuKind::Abilities => &["(Abilities — data pending)"],
        MenuKind::Items => ITEMS_ENTRIES_STUB,
        // Stage 1: 16 slot-name labels. Per-frame `update_main_menu`
        // appends the equipped item name (or "—" if empty) from
        // `SceneSnapshot.equipped[i]` — the slot-name slice gives the
        // cursor + count, the snapshot gives the right column.
        MenuKind::Equipment => EQUIPMENT_ENTRIES,
    }
}

/// Marker on the menu root.
#[derive(Component)]
pub struct MainMenu;

/// Marker on each menu row. `slot` is the row index 0..entries.len().
#[derive(Component)]
pub struct MainMenuRow {
    pub slot: usize,
}

/// Emitted when an operator clicks (LMB-press) a menu row. The dispatch
/// consumer in `ffxi-client/src/view_native/text_input.rs` reads this
/// alongside its keyboard handler so mouse and keyboard share the same
/// activation path (`resolve_menu_entry` for the current `MenuKind`).
#[derive(Message, Debug, Clone, Copy)]
pub struct MenuRowActivated {
    pub slot: usize,
}

pub fn spawn_main_menu(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            MainMenu,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                right: Val::Px(8.0),
                // 240px fits "Body  : Bronze Cuirass +1" without
                // truncating the longer FFXI item names. Static
                // submenus (Root/Config/Graphics) leave the right
                // side empty padding — that's fine; the panel
                // shrinks visually via Display::None on hidden rows.
                width: Val::Px(240.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            // Spawn the maximum-sized row pool once. The update system
            // hides rows past `entry_count(kind)` per frame so smaller
            // submenus (Config has 5 entries, Root has 9) don't leave
            // ghost rows visible.
            for slot in 0..MAX_ENTRY_COUNT {
                p.spawn((
                    MainMenuRow { slot },
                    // `Button` opts the row into Bevy UI's Interaction
                    // tracking — the cursor module reads Hovered/Pressed
                    // to swap to the Hand sprite, and the click + hover
                    // systems below dispatch to the shared menu handler.
                    Button,
                    Text::new(""),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
            }
        });
}

/// Stage-2 refresh: rebuild [`DynamicMenu`] from the snapshot's
/// learned-spell / known-ability mirrors. Runs every frame in
/// `Update`; cheap (touches ~hundreds of u16s, allocates strings only
/// when an id is in the table). Keeping it idempotent + change-detect-
/// free avoids the failure mode where a stale row persists after
/// a job change but before the Bevy change-detection tick.
pub fn refresh_dynamic_menu_rows(
    mode: Res<InputMode>,
    scene: Res<crate::snapshot::SceneState>,
    mut dynamic: ResMut<DynamicMenu>,
) {
    let active_kind = match &*mode {
        InputMode::Menu(stack) => stack.current().map(|l| l.kind),
        _ => None,
    };
    // Out of a menu, or in a static menu — clear dynamic state so the
    // next dynamic open starts fresh.
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
            // Concatenate the three categories. v1 ships a flat list
            // sorted by category then name; subtab cycling can land
            // later (see plan §"subtabs" — flat for now per the
            // approved scope).
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
            .inventory_main
            .iter()
            .filter_map(|slot| {
                let name = ffxi_proto::item_names::lookup(slot.item_no)?;
                // Show quantity for stackable items (qty > 1) so the
                // operator can see at-a-glance whether they have
                // multiple Echo Drops to spare. Single-quantity items
                // render bare to match retail's compact look.
                let label = if slot.quantity > 1 {
                    format!("{name} x{}", slot.quantity)
                } else {
                    name.to_string()
                };
                Some(DynamicMenuRow {
                    label,
                    action: DynamicMenuAction::UseItem {
                        container: slot.container,
                        index: slot.index,
                        item_no: slot.item_no,
                    },
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    // Sort alphabetically within a category by label — retail does
    // category-then-name; v1 just does name (subtabs land later).
    let mut rows = rows;
    rows.sort_by(|a, b| a.label.cmp(&b.label));
    if rows != dynamic.rows {
        dynamic.rows = rows;
    }
}

/// Per-frame: toggle visibility, update cursor highlighting, swap entry
/// labels when the active `MenuKind` changes (e.g. Root → Config).
pub fn update_main_menu(
    mode: Res<InputMode>,
    settings: Res<GraphicsSettings>,
    // Equipment rows pull the equipped item name from the snapshot at
    // render time. Stage 0 menus don't need this; Stage 1 added it for
    // the new `MenuKind::Equipment` branch in `format_row_body`.
    scene: Res<crate::snapshot::SceneState>,
    dynamic: Res<DynamicMenu>,
    mut menu_q: Query<&mut Node, (With<MainMenu>, Without<MainMenuRow>)>,
    mut row_q: Query<(&MainMenuRow, &mut Node, &mut Text, &mut TextColor)>,
) {
    let Ok(mut node) = menu_q.single_mut() else {
        return;
    };

    // Pull the active screen + cursor from the menu stack's *top* frame.
    // `stack.current()` is the deepest-pushed menu — Root by default,
    // Config after the operator selects "Config" from Root.
    let active: Option<(MenuKind, usize)> = match &*mode {
        InputMode::Menu(stack) => stack.current().map(|l| (l.kind, l.cursor)),
        _ => None,
    };

    match active {
        Some((kind, c)) => {
            node.display = Display::Flex;
            // Dynamic submenus (Magic / Abilities) get viewport
            // windowing — the row pool size is fixed, the list can be
            // larger, so we slide a window of `DYNAMIC_VISIBLE_ROWS`
            // around the cursor. Static menus return their natural
            // slice and `viewport_start = 0`.
            let (total_count, viewport_start) =
                resolve_viewport(kind, c, &dynamic);
            for (row, mut row_node, mut text, mut color) in row_q.iter_mut() {
                // Map this row's pool index → list index via the
                // viewport offset. Rows beyond the visible window
                // hide.
                let list_idx = viewport_start + row.slot;
                let visible = row.slot < DYNAMIC_VISIBLE_ROWS && list_idx < total_count;
                if !visible {
                    if row_node.display != Display::None {
                        row_node.display = Display::None;
                    }
                    continue;
                }
                let label_owned: String = if is_dynamic(kind) {
                    // Pull from DynamicMenu; the placeholder hint
                    // surfaces when the list is empty.
                    entry_label(kind, list_idx, &dynamic).to_string()
                } else {
                    static_entries(kind)
                        .get(list_idx)
                        .copied()
                        .unwrap_or("<unknown>")
                        .to_string()
                };
                if row_node.display != Display::Flex {
                    row_node.display = Display::Flex;
                }
                let is_cursor = list_idx == c;
                let body = format_row_body(
                    kind,
                    list_idx,
                    &label_owned,
                    &settings,
                    &scene.snapshot,
                );
                let want = if is_cursor {
                    format!("> {body}")
                } else {
                    format!("  {body}")
                };
                if **text != want {
                    **text = want;
                }
                let want_color = if is_cursor {
                    palette::ACCENT
                } else {
                    palette::MUTED
                };
                if color.0 != want_color {
                    color.0 = want_color;
                }
            }
        }
        None => {
            if node.display != Display::None {
                node.display = Display::None;
            }
        }
    }
}

/// Compute `(total_list_len, viewport_start)` for the active submenu.
/// Static menus return `(static_entries(kind).len(), 0)` — they fit in
/// the row pool, so no windowing needed. Dynamic menus center the
/// viewport on the cursor when the list overflows
/// [`DYNAMIC_VISIBLE_ROWS`].
fn resolve_viewport(kind: MenuKind, cursor: usize, dynamic: &DynamicMenu) -> (usize, usize) {
    if is_dynamic(kind) {
        let total = dynamic.rows.len().max(1);
        if total <= DYNAMIC_VISIBLE_ROWS {
            return (total, 0);
        }
        // Center the cursor inside the window, then clamp to the list
        // boundaries so the last visible row never extends past the
        // end of the list.
        let half = DYNAMIC_VISIBLE_ROWS / 2;
        let max_start = total.saturating_sub(DYNAMIC_VISIBLE_ROWS);
        let start = cursor.saturating_sub(half).min(max_start);
        (total, start)
    } else {
        (static_entries(kind).len(), 0)
    }
}

/// Move the menu cursor to follow mouse hover. Runs only while
/// `InputMode::Menu` is active; outside that mode the rows are
/// `Display::None` so their `Interaction` never updates anyway.
pub fn menu_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    dynamic: Res<DynamicMenu>,
    rows: Query<(&Interaction, &MainMenuRow), Changed<Interaction>>,
) {
    let InputMode::Menu(stack) = &mut *mode else {
        return;
    };
    let Some(level) = stack.current_mut() else {
        return;
    };
    let kind = level.kind;
    // For dynamic submenus, the row pool index is relative to the
    // viewport; translate back to a list index before applying as the
    // cursor.
    let (total, viewport_start) = resolve_viewport(kind, level.cursor, &dynamic);
    for (interaction, row) in &rows {
        if !matches!(interaction, Interaction::Hovered | Interaction::Pressed) {
            continue;
        }
        let list_idx = viewport_start + row.slot;
        if list_idx >= total || row.slot >= DYNAMIC_VISIBLE_ROWS {
            continue;
        }
        if level.cursor != list_idx {
            level.cursor = list_idx;
        }
    }
}

/// Emit [`MenuRowActivated`] when a row is pressed. The `slot` carried
/// in the message is the *list index* (not the pool row index) so
/// downstream dispatchers can read directly from the dynamic-menu
/// resource without a viewport translation step.
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
    let (total, viewport_start) = resolve_viewport(kind, level.cursor, &dynamic);
    for (interaction, row) in &rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let list_idx = viewport_start + row.slot;
        if list_idx < total && row.slot < DYNAMIC_VISIBLE_ROWS {
            out.write(MenuRowActivated { slot: list_idx });
        }
    }
}

/// Format the body of a menu row (everything after the cursor prefix).
/// Graphics field rows render `Field: [Value]`; Equipment slot rows
/// render `Slot: item_name`; every other screen renders the bare label.
fn format_row_body(
    kind: MenuKind,
    slot: usize,
    label: &str,
    settings: &GraphicsSettings,
    snapshot: &ffxi_viewer_wire::SceneSnapshot,
) -> String {
    match kind {
        MenuKind::Graphics => match GRAPHICS_FIELDS.get(slot).copied() {
            Some(field) => format!(
                "{:<16}[{}]",
                format!("{}:", field.label()),
                settings.value_label(field)
            ),
            // Reset row (and any future trailing actions).
            None => label.to_string(),
        },
        MenuKind::Equipment => {
            // Two failure modes both collapse to "—":
            //   (1) slot is genuinely empty
            //   (2) we received `EQUIP_LIST` (container,index) before
            //       the inventory flood resolved that slot — wire_translate
            //       writes None until inventory catches up
            // Stage 1 doesn't disambiguate; a real "loading" indicator
            // can come in a later stage if it proves confusing.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// GRAPHICS_ENTRIES is the dispatcher's source of truth for routing
    /// (text_input matches on these labels), and GraphicsField::label()
    /// is the renderer's source of truth for the visible "Field:" text
    /// — they must agree for every row.
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
}
