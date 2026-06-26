use bevy::prelude::*;

use crate::graphics_settings::{GraphicsSettings, GRAPHICS_FIELDS};
use crate::hud::palette;
use crate::input_mode::{InputMode, MenuKind};

const ROOT_ENTRIES: &[&str] = &[
    "Magic",
    "Abilities",
    "Items",
    "Key Items",
    "Equipment",
    "Status",
    "Party",
    "Search",
    "Macros",
    "Graphics",
    "Config",
    "Logout",
];

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

    UseItem {
        container: u8,
        index: u8,
        item_no: u16,
    },

    EquipItem {
        container: u8,
        container_index: u8,
        equip_slot: u8,
    },
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
    "Dynamic Lights",
    "  Threshold",
    "  Intensity",
    "  Range",
    "  Flicker",
    "Model Lighting",
    "Model Shadows",
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
        MenuKind::Magic | MenuKind::Abilities | MenuKind::Items | MenuKind::EquipSlot(_)
    )
}

pub fn entry_count(kind: MenuKind, dynamic: &DynamicMenu) -> usize {
    if is_dynamic(kind) {
        dynamic.rows.len().max(1)
    } else {
        static_entries(kind).len()
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
        MenuKind::Items => "(inventory empty)",
        MenuKind::EquipSlot(_) => "(no equippable items for this slot)",
        _ => "(empty)",
    }
}

fn static_entries(kind: MenuKind) -> &'static [&'static str] {
    match kind {
        MenuKind::Root => ROOT_ENTRIES,
        MenuKind::Config => CONFIG_ENTRIES,
        MenuKind::Graphics => GRAPHICS_ENTRIES,

        MenuKind::Magic => &["(Magic — data pending)"],
        MenuKind::Abilities => &["(Abilities — data pending)"],
        MenuKind::Items => ITEMS_ENTRIES_STUB,

        MenuKind::Equipment => EQUIPMENT_ENTRIES,

        MenuKind::Status => STATUS_LABELS,

        MenuKind::EquipSlot(_) => &["(loading equippable items…)"],
    }
}

fn menu_title(kind: MenuKind) -> &'static str {
    match kind {
        MenuKind::Root => "Commands",
        MenuKind::Config => "Config",
        MenuKind::Graphics => "Graphics",
        MenuKind::Equipment => "Equipment",
        MenuKind::Magic => "Magic",
        MenuKind::Abilities => "Abilities",
        MenuKind::Items => "Items",
        MenuKind::Status => "Status",
        MenuKind::EquipSlot(_) => "Equip",
    }
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

pub fn spawn_main_menu(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            MainMenu,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                right: Val::Px(8.0),

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
            p.spawn((
                MainMenuTitle,
                Text::new(""),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));

            for slot in 0..MAX_ENTRY_COUNT {
                p.spawn((
                    MainMenuRow { slot },
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

pub fn refresh_dynamic_menu_rows(
    mode: Res<InputMode>,
    scene: Res<crate::snapshot::SceneState>,
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
            .inventory_main
            .iter()
            .filter_map(|slot| {
                let name = ffxi_proto::item_names::lookup(slot.item_no)?;

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
        MenuKind::EquipSlot(equip_slot) => {
            let (main_job, main_lv) = snap
                .self_char_id
                .and_then(|id| snap.party.iter().find(|m| m.id == id))
                .map(|m| (m.main_job, m.main_job_lv))
                .unwrap_or((0, 0));
            snap.inventory_main
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
                        },
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    };

    let mut rows = rows;
    rows.sort_by(|a, b| a.label.cmp(&b.label));
    if rows != dynamic.rows {
        dynamic.rows = rows;
    }
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
        G::RangedAttack | G::Mount => Vec::new(),
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

pub fn update_main_menu(
    mode: Res<InputMode>,
    settings: Res<GraphicsSettings>,

    scene: Res<crate::snapshot::SceneState>,
    dynamic: Res<DynamicMenu>,
    mut menu_q: Query<&mut Node, (With<MainMenu>, Without<MainMenuRow>)>,
    mut row_q: Query<(&MainMenuRow, &mut Node, &mut Text, &mut TextColor), Without<MainMenuTitle>>,
    mut title_q: Query<&mut Text, (With<MainMenuTitle>, Without<MainMenuRow>)>,
) {
    let Ok(mut node) = menu_q.single_mut() else {
        return;
    };

    let active: Option<(MenuKind, usize)> = match &*mode {
        InputMode::Menu(stack) => stack.current().map(|l| (l.kind, l.cursor)),
        _ => None,
    };

    match active {
        Some((kind, c)) => {
            node.display = Display::Flex;

            if let Ok(mut text) = title_q.single_mut() {
                let want = menu_title(kind);
                if **text != *want {
                    **text = want.to_string();
                }
            }

            let (total_count, viewport_start) = resolve_viewport(kind, c, &dynamic);

            let window = visible_window(kind, total_count);
            for (row, mut row_node, mut text, mut color) in row_q.iter_mut() {
                let list_idx = viewport_start + row.slot;
                let visible = row.slot < window && list_idx < total_count;
                if !visible {
                    if row_node.display != Display::None {
                        row_node.display = Display::None;
                    }
                    continue;
                }
                let label_owned: String = if is_dynamic(kind) {
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
                let body =
                    format_row_body(kind, list_idx, &label_owned, &settings, &scene.snapshot);
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

fn visible_window(kind: MenuKind, total: usize) -> usize {
    if is_dynamic(kind) {
        DYNAMIC_VISIBLE_ROWS
    } else {
        total
    }
}

fn resolve_viewport(kind: MenuKind, cursor: usize, dynamic: &DynamicMenu) -> (usize, usize) {
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
    let Some(level) = stack.current_mut() else {
        return;
    };
    let kind = level.kind;

    let (total, viewport_start) = resolve_viewport(kind, level.cursor, &dynamic);
    let window = visible_window(kind, total);
    for (interaction, row) in &rows {
        if !matches!(interaction, Interaction::Hovered | Interaction::Pressed) {
            continue;
        }
        let list_idx = viewport_start + row.slot;
        if list_idx >= total || row.slot >= window {
            continue;
        }
        if level.cursor != list_idx {
            level.cursor = list_idx;
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
    let (total, viewport_start) = resolve_viewport(kind, level.cursor, &dynamic);
    let window = visible_window(kind, total);
    for (interaction, row) in &rows {
        if *interaction != Interaction::Pressed {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
            MenuKind::Graphics,
            MenuKind::Equipment,
            MenuKind::Status,
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
