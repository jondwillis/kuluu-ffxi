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

/// Placeholder rows shown for the four action submenus during Stage 0
/// of the menu rollout. Each submenu replaces this slice with its real
/// data source in a later stage (`hud/equipment_menu.rs` in Stage 1,
/// etc.). One row keeps the cursor logic well-defined while the
/// renderer is the existing `update_main_menu` (no scrollable list
/// widget needed yet).
const MAGIC_ENTRIES_STUB: &[&str] = &["(Magic — Stage 2: pending learned-spell decoder)"];
const ABILITIES_ENTRIES_STUB: &[&str] =
    &["(Abilities — Stage 2: pending s2c 0x119 abil_recast decoder)"];
const ITEMS_ENTRIES_STUB: &[&str] = &["(Items — Stage 3: pending inventory submenu)"];

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
    // Equipment's 16 slots are the largest fixed submenu; Magic /
    // Abilities / Items stubs are still 1 row each. Stages 2+ that
    // need long scrollable lists will spawn their own panel (planned
    // `hud/menu_list.rs`) rather than keep expanding this pool.
    let rc = if r >= c { r } else { c };
    let rcg = if rc >= g { rc } else { g };
    if rcg >= e {
        rcg
    } else {
        e
    }
};

/// Number of entries on the named menu screen. Used by the input router
/// to clamp cursor movement.
pub fn entry_count(kind: MenuKind) -> usize {
    entries(kind).len()
}

/// Label for a given menu screen + cursor index. Out-of-range returns
/// `"<unknown>"` rather than panicking, since the input router clamps
/// cursor bounds independently and a stale index would otherwise crash.
pub fn entry_label(kind: MenuKind, idx: usize) -> &'static str {
    entries(kind).get(idx).copied().unwrap_or("<unknown>")
}

fn entries(kind: MenuKind) -> &'static [&'static str] {
    match kind {
        MenuKind::Root => ROOT_ENTRIES,
        MenuKind::Config => CONFIG_ENTRIES,
        MenuKind::Graphics => GRAPHICS_ENTRIES,
        MenuKind::Magic => MAGIC_ENTRIES_STUB,
        MenuKind::Abilities => ABILITIES_ENTRIES_STUB,
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

/// Per-frame: toggle visibility, update cursor highlighting, swap entry
/// labels when the active `MenuKind` changes (e.g. Root → Config).
pub fn update_main_menu(
    mode: Res<InputMode>,
    settings: Res<GraphicsSettings>,
    // Equipment rows pull the equipped item name from the snapshot at
    // render time. Stage 0 menus don't need this; Stage 1 added it for
    // the new `MenuKind::Equipment` branch in `format_row_body`.
    scene: Res<crate::snapshot::SceneState>,
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
            let labels = entries(kind);
            for (row, mut row_node, mut text, mut color) in row_q.iter_mut() {
                match labels.get(row.slot).copied() {
                    Some(label) => {
                        // Slot is in-range for the active screen — make
                        // sure it's visible and rendering the cursor or
                        // the label as appropriate.
                        if row_node.display != Display::Flex {
                            row_node.display = Display::Flex;
                        }
                        let is_cursor = row.slot == c;
                        // Graphics rows are `Field: [Value]` (with the
                        // value pulled from the live settings resource).
                        // Reset and other screens use the plain static
                        // label.
                        let body =
                            format_row_body(kind, row.slot, label, &settings, &scene.snapshot);
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
                    None => {
                        // Slot is past the active screen's entry count —
                        // hide so the panel doesn't reserve space for
                        // empty rows beneath the last visible entry.
                        if row_node.display != Display::None {
                            row_node.display = Display::None;
                        }
                    }
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

/// Move the menu cursor to follow mouse hover. Runs only while
/// `InputMode::Menu` is active; outside that mode the rows are
/// `Display::None` so their `Interaction` never updates anyway.
pub fn menu_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    rows: Query<(&Interaction, &MainMenuRow), Changed<Interaction>>,
) {
    let InputMode::Menu(stack) = &mut *mode else {
        return;
    };
    let Some(level) = stack.current_mut() else {
        return;
    };
    let limit = entry_count(level.kind);
    for (interaction, row) in &rows {
        if matches!(interaction, Interaction::Hovered | Interaction::Pressed)
            && row.slot < limit
            && level.cursor != row.slot
        {
            level.cursor = row.slot;
        }
    }
}

/// Emit [`MenuRowActivated`] when a row is pressed. Filtered to in-bounds
/// slots so a click on a hidden trailing row (the spawn pool always has
/// `MAX_ENTRY_COUNT` rows but smaller submenus hide the extras) doesn't
/// dispatch a `<unknown>` label downstream.
pub fn menu_mouse_click_system(
    mode: Res<InputMode>,
    rows: Query<(&Interaction, &MainMenuRow), Changed<Interaction>>,
    mut out: MessageWriter<MenuRowActivated>,
) {
    let InputMode::Menu(stack) = &*mode else {
        return;
    };
    let Some(level) = stack.current() else {
        return;
    };
    let limit = entry_count(level.kind);
    for (interaction, row) in &rows {
        if *interaction == Interaction::Pressed && row.slot < limit {
            out.write(MenuRowActivated { slot: row.slot });
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
