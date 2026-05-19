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
    "Status",
    "Party",
    "Search",
    "Macros",
    "Graphics",
    "Config",
    "Logout",
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
    let rc = if r >= c { r } else { c };
    if rc >= g {
        rc
    } else {
        g
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

pub fn spawn_main_menu(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            MainMenu,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                right: Val::Px(8.0),
                width: Val::Px(160.0),
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
                        let body = format_row_body(kind, row.slot, label, &settings);
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

/// Format the body of a menu row (everything after the cursor prefix).
/// Graphics field rows render `Field: [Value]`; the trailing "Reset to
/// High" row and every non-Graphics row render the bare label.
fn format_row_body(
    kind: MenuKind,
    slot: usize,
    label: &str,
    settings: &GraphicsSettings,
) -> String {
    if !matches!(kind, MenuKind::Graphics) {
        return label.to_string();
    }
    match GRAPHICS_FIELDS.get(slot).copied() {
        Some(field) => format!(
            "{:<16}[{}]",
            format!("{}:", field.label()),
            settings.value_label(field)
        ),
        // Reset row (and any future trailing actions).
        None => label.to_string(),
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
