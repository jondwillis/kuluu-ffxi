use bevy::prelude::*;

use crate::hud::item_dat_root::{ItemDatRoot, ItemIconCache};
use crate::hud::item_ui::{self, framed_box, text_font, theme, transparent_placeholder};
use crate::hud::status_panel;
use crate::input_mode::{InputMode, MenuKind};
use crate::snapshot::SceneState;

pub const SLOT_NAMES: [&str; 16] = [
    "Main", "Sub", "Ranged", "Ammo", "Head", "Body", "Hands", "Legs", "Feet", "Neck", "Waist",
    "L.Ear", "R.Ear", "L.Ring", "R.Ring", "Back",
];

const SLOT_ABBR: [&str; 16] = [
    "Main", "Sub", "Rng", "Amo", "Head", "Body", "Hnds", "Legs", "Feet", "Neck", "Wst", "L.Er",
    "R.Er", "L.Rg", "R.Rg", "Back",
];

// Discriminants are the internal slot indices used by SceneSnapshot.equipped[16]
// and MenuKind::EquipSlot — `repr(u8)` lets `slot as usize` recover that index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EquipmentIndex {
    Main = 0,
    Sub = 1,
    Range = 2,
    Ammo = 3,
    Head = 4,
    Body = 5,
    Hands = 6,
    Legs = 7,
    Feet = 8,
    Neck = 9,
    Waist = 10,
    LeftEar = 11,
    RightEar = 12,
    LeftRing = 13,
    RightRing = 14,
    Back = 15,
}

impl EquipmentIndex {
    pub const ALL: [EquipmentIndex; 16] = [
        Self::Main,
        Self::Sub,
        Self::Range,
        Self::Ammo,
        Self::Head,
        Self::Body,
        Self::Hands,
        Self::Legs,
        Self::Feet,
        Self::Neck,
        Self::Waist,
        Self::LeftEar,
        Self::RightEar,
        Self::LeftRing,
        Self::RightRing,
        Self::Back,
    ];

    pub fn from_index(index: u8) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }

    pub fn name(self) -> &'static str {
        SLOT_NAMES[self as usize]
    }

    pub fn abbr(self) -> &'static str {
        SLOT_ABBR[self as usize]
    }
}

// Retail equipment-window grid, one group per row (ref: kuluu-y04 retail
// screenshot): weapons / head-neck-ears / body-hands-rings / back-waist-legs-feet.
pub const EQUIP_GRID: [[EquipmentIndex; 4]; 4] = {
    use EquipmentIndex::*;
    [
        [Main, Sub, Range, Ammo],
        [Head, Neck, LeftEar, RightEar],
        [Body, Hands, LeftRing, RightRing],
        [Back, Waist, Legs, Feet],
    ]
};

fn slot_to_cell(slot: EquipmentIndex) -> (usize, usize) {
    for (r, row) in EQUIP_GRID.iter().enumerate() {
        for (c, &s) in row.iter().enumerate() {
            if s == slot {
                return (r, c);
            }
        }
    }
    (0, 0)
}

/// Move the grid cursor (an internal slot index) by `dx` columns / `dy` rows,
/// wrapping like the retail window. Used by the menu key handler.
pub fn grid_move(slot: u8, dx: i32, dy: i32) -> u8 {
    let slot = EquipmentIndex::from_index(slot).unwrap_or(EquipmentIndex::Main);
    let (r, c) = slot_to_cell(slot);
    let nr = (r as i32 + dy).rem_euclid(EQUIP_GRID.len() as i32) as usize;
    let nc = (c as i32 + dx).rem_euclid(EQUIP_GRID[0].len() as i32) as usize;
    EQUIP_GRID[nr][nc] as u8
}

const DETAIL_ROWS: usize = 10;
const STORAGE_ROWS: usize = 16;
const CELL_PX: f32 = 36.0;
const ICON_PX: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EquipRole {
    Header,
    StatusName,
    StatusVitals,
    StatusILvl,
    StatusAttr(usize),
    StatusCombat,
    CellLabel(EquipmentIndex),
    DetailName,
    DetailRow(usize),
    StorageTitle,
    StorageRow(usize),
}

#[derive(Component, Clone, Copy)]
pub(crate) struct EquipText(EquipRole);

#[derive(Clone, Copy, PartialEq, Eq)]
enum IconSlot {
    Cell(EquipmentIndex),
    Detail,
}

#[derive(Component, Clone, Copy)]
pub(crate) struct EquipIcon(IconSlot);

#[derive(Component, Clone, Copy)]
pub(crate) struct EquipCellFrame(EquipmentIndex);

#[derive(Component)]
pub(crate) struct EquipScreenRoot;

#[derive(Component)]
pub(crate) struct EquipStorageBox;

enum ScreenState {
    Closed,
    SlotPicker {
        selected_slot: EquipmentIndex,
    },
    StoragePicker {
        selected_slot: EquipmentIndex,
        storage_cursor: usize,
    },
}

fn screen_state(mode: &InputMode) -> ScreenState {
    let InputMode::Menu(stack) = mode else {
        return ScreenState::Closed;
    };
    match stack.current() {
        Some(level) => match level.kind {
            MenuKind::Equipment => ScreenState::SlotPicker {
                selected_slot: EquipmentIndex::from_index(level.cursor as u8)
                    .unwrap_or(EquipmentIndex::Main),
            },
            MenuKind::EquipSlot(slot) => ScreenState::StoragePicker {
                selected_slot: EquipmentIndex::from_index(slot).unwrap_or(EquipmentIndex::Main),
                storage_cursor: level.cursor,
            },
            _ => ScreenState::Closed,
        },
        None => ScreenState::Closed,
    }
}

pub(crate) fn spawn_equipment_screen(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let placeholder = transparent_placeholder(&mut images);

    commands
        .spawn((
            crate::components::InGameEntity,
            EquipScreenRoot,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(8.0),
                column_gap: Val::Px(6.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                display: Display::None,
                ..default()
            },
        ))
        .with_children(|root| {
            // Left column: Status panel above the item-detail panel.
            root.spawn(Node {
                width: Val::Px(264.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|col| {
                let (mut n, bg, bd) = framed_box();
                n.min_width = Val::Px(264.0);
                col.spawn((n, bg, bd)).with_children(|p| {
                    spawn_text(p, EquipRole::StatusName, 14.0, theme::TITLE);
                    spawn_text(p, EquipRole::StatusVitals, 13.0, theme::TEXT);
                    spawn_text(p, EquipRole::StatusILvl, 12.0, theme::MUTED);
                    for i in 0..status_panel::PRIMARY_ATTRS.len() {
                        spawn_text(p, EquipRole::StatusAttr(i), 13.0, theme::TEXT);
                    }
                    spawn_text(p, EquipRole::StatusCombat, 13.0, theme::TEXT);
                });

                let (n, bg, bd) = framed_box();
                col.spawn((n, bg, bd)).with_children(|p| {
                    p.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(6.0),
                        ..default()
                    })
                    .with_children(|h| {
                        h.spawn((
                            EquipIcon(IconSlot::Detail),
                            Node {
                                width: Val::Px(32.0),
                                height: Val::Px(32.0),
                                display: Display::None,
                                ..default()
                            },
                            ImageNode::new(placeholder.clone()),
                        ));
                        h.spawn((
                            EquipText(EquipRole::DetailName),
                            Text::new(""),
                            text_font(14.0),
                            TextColor(theme::TITLE),
                        ));
                    });
                    for i in 0..DETAIL_ROWS {
                        spawn_row(p, EquipRole::DetailRow(i), 12.0, theme::TEXT);
                    }
                });
            });

            // Center: title + 4x4 equipment-slot icon grid.
            let (n, bg, bd) = framed_box();
            root.spawn((n, bg, bd)).with_children(|p| {
                spawn_text(p, EquipRole::Header, 14.0, theme::TITLE);
                p.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    margin: UiRect::top(Val::Px(4.0)),
                    ..default()
                })
                .with_children(|grid| {
                    for row in EQUIP_GRID.iter() {
                        grid.spawn(Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(4.0),
                            ..default()
                        })
                        .with_children(|line| {
                            for &slot in row.iter() {
                                spawn_cell(line, slot, placeholder.clone());
                            }
                        });
                    }
                });
            });

            // Right: equippable-item storage list (only shown while picking).
            let (mut n, bg, bd) = framed_box();
            n.width = Val::Px(204.0);
            root.spawn((EquipStorageBox, n, bg, bd)).with_children(|p| {
                spawn_text(p, EquipRole::StorageTitle, 13.0, theme::TITLE);
                for i in 0..STORAGE_ROWS {
                    spawn_row(p, EquipRole::StorageRow(i), 12.0, theme::TEXT);
                }
            });
        });
}

fn spawn_text(p: &mut ChildSpawnerCommands, role: EquipRole, size: f32, color: Color) {
    p.spawn((
        EquipText(role),
        Text::new(""),
        text_font(size),
        TextColor(color),
    ));
}

fn spawn_row(p: &mut ChildSpawnerCommands, role: EquipRole, size: f32, color: Color) {
    p.spawn((
        EquipText(role),
        Text::new(""),
        text_font(size),
        TextColor(color),
        Node {
            display: Display::None,
            ..default()
        },
    ));
}

fn spawn_cell(p: &mut ChildSpawnerCommands, slot: EquipmentIndex, placeholder: Handle<Image>) {
    p.spawn((
        EquipCellFrame(slot),
        Node {
            width: Val::Px(CELL_PX),
            height: Val::Px(CELL_PX),
            border: UiRect::all(Val::Px(1.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(theme::CELL_BG),
        BorderColor::all(theme::CELL_EDGE),
    ))
    .with_children(|c| {
        c.spawn((
            EquipIcon(IconSlot::Cell(slot)),
            Node {
                width: Val::Px(ICON_PX),
                height: Val::Px(ICON_PX),
                display: Display::None,
                ..default()
            },
            ImageNode::new(placeholder),
        ));
        c.spawn((
            EquipText(EquipRole::CellLabel(slot)),
            Text::new(slot.abbr()),
            text_font(11.0),
            TextColor(theme::MUTED),
        ));
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_equipment_screen(
    mode: Res<InputMode>,
    state: Res<SceneState>,
    dynamic: Res<crate::hud::menu::DynamicMenu>,
    dat_root: Res<ItemDatRoot>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut root_q: Query<
        &mut Node,
        (
            With<EquipScreenRoot>,
            Without<EquipText>,
            Without<EquipIcon>,
            Without<EquipStorageBox>,
        ),
    >,
    mut storage_q: Query<
        &mut Node,
        (
            With<EquipStorageBox>,
            Without<EquipScreenRoot>,
            Without<EquipText>,
            Without<EquipIcon>,
        ),
    >,
    mut text_q: Query<
        (&EquipText, &mut Text, &mut TextColor, &mut Node),
        (
            Without<EquipScreenRoot>,
            Without<EquipIcon>,
            Without<EquipStorageBox>,
        ),
    >,
    mut icon_q: Query<
        (&EquipIcon, &mut Node, &mut ImageNode),
        (
            Without<EquipScreenRoot>,
            Without<EquipText>,
            Without<EquipStorageBox>,
        ),
    >,
    mut cell_q: Query<(&EquipCellFrame, &mut BorderColor, &mut BackgroundColor)>,
) {
    let st = screen_state(&mode);
    let (selected_slot, storage_active, storage_cursor) = match st {
        ScreenState::Closed => {
            if let Ok(mut node) = root_q.single_mut() {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
            return;
        }
        ScreenState::SlotPicker { selected_slot } => (selected_slot, false, 0),
        ScreenState::StoragePicker {
            selected_slot,
            storage_cursor,
        } => (selected_slot, true, storage_cursor),
    };

    if let Ok(mut node) = root_q.single_mut() {
        if node.display != Display::Flex {
            node.display = Display::Flex;
        }
    }
    if let Ok(mut node) = storage_q.single_mut() {
        let want = if storage_active {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }

    let snap = &state.snapshot;
    let me = crate::hud::self_hud::resolve_self(&snap.party, snap.self_char_id);

    let equipped = |slot: EquipmentIndex| -> Option<u16> {
        snap.equipped.get(slot as usize).copied().flatten()
    };

    let focused_item: Option<u16> = if storage_active {
        dynamic
            .rows
            .get(storage_cursor)
            .and_then(|r| match r.action {
                crate::hud::menu::DynamicMenuAction::EquipItem { item_no, .. } => Some(item_no),
                _ => None,
            })
    } else {
        equipped(selected_slot)
    };

    let (detail_name, detail_rows) =
        item_ui::focus_detail(focused_item, snap, &dat_root, &mut icon_cache);

    // Storage list viewport (keep the cursor in view).
    let storage_total = dynamic.rows.len();
    let storage_start = storage_cursor
        .saturating_sub(STORAGE_ROWS / 2)
        .min(storage_total.saturating_sub(STORAGE_ROWS));

    for (tag, mut text, mut color, mut node) in text_q.iter_mut() {
        let (want, want_color, visible) = role_value(
            tag.0,
            snap,
            me,
            selected_slot,
            storage_active,
            &detail_name,
            &detail_rows,
            &dynamic,
            storage_cursor,
            storage_start,
        );
        let display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
        if visible && **text != want {
            **text = want;
        }
        if color.0 != want_color {
            color.0 = want_color;
        }
    }

    for (icon, mut node, mut image) in icon_q.iter_mut() {
        let item = match icon.0 {
            IconSlot::Cell(slot) => equipped(slot),
            IconSlot::Detail => focused_item,
        };
        let handle = item.and_then(|n| icon_cache.ensure(n, &dat_root, &mut images));
        match handle {
            Some(h) => {
                if image.image != h {
                    image.image = h;
                }
                if image.color != Color::WHITE {
                    image.color = Color::WHITE;
                }
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }

    for (cell, mut border, mut bg) in cell_q.iter_mut() {
        let focused = cell.0 == selected_slot;
        let want_border = if focused {
            theme::CURSOR
        } else {
            theme::CELL_EDGE
        };
        if border.left != want_border {
            *border = BorderColor::all(want_border);
        }
        let want_bg = if focused {
            theme::CURSOR_BG
        } else {
            theme::CELL_BG
        };
        if bg.0 != want_bg {
            bg.0 = want_bg;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn role_value(
    role: EquipRole,
    snap: &ffxi_viewer_wire::SceneSnapshot,
    me: Option<&ffxi_viewer_wire::PartyMember>,
    selected_slot: EquipmentIndex,
    storage_active: bool,
    detail_name: &str,
    detail_rows: &[String],
    dynamic: &crate::hud::menu::DynamicMenu,
    storage_cursor: usize,
    storage_start: usize,
) -> (String, Color, bool) {
    let attr = |i: usize| -> Option<u16> {
        snap.stats.as_ref().map(|s| match i {
            0 => s.str_,
            1 => s.dex,
            2 => s.vit,
            3 => s.agi,
            4 => s.int_,
            5 => s.mnd,
            _ => s.chr,
        })
    };
    match role {
        EquipRole::Header => {
            let item = snap
                .equipped
                .get(selected_slot as usize)
                .copied()
                .flatten()
                .and_then(ffxi_proto::item_names::lookup)
                .unwrap_or("(empty)");
            (
                format!("{}: {item}", selected_slot.name()),
                theme::TITLE,
                true,
            )
        }
        EquipRole::StatusName => (profile_line(snap, me), theme::TITLE, true),
        EquipRole::StatusVitals => {
            let s = match me {
                Some(m) => match snap.stats.as_ref().filter(|s| s.hp_max > 0) {
                    Some(st) => format!(
                        "HP {}/{}  MP {}/{}  TP {}",
                        m.hp, st.hp_max, m.mp, st.mp_max, m.tp
                    ),
                    None => format!("HP {}   MP {}   TP {}", m.hp, m.mp, m.tp),
                },
                None => "HP —   MP —   TP —".to_string(),
            };
            (s, theme::TEXT, true)
        }
        EquipRole::StatusILvl => {
            let s = match snap.stats.as_ref().filter(|s| s.item_level > 0) {
                Some(st) => format!("Item Level: {}", st.item_level),
                None => "Item Level: —".to_string(),
            };
            (s, theme::MUTED, true)
        }
        EquipRole::StatusAttr(i) => {
            let name = status_panel::PRIMARY_ATTRS.get(i).copied().unwrap_or("");
            let s = match (attr(i), snap.stats.as_ref()) {
                (Some(base), Some(st)) => {
                    let bonus = st.bonus.get(i).copied().unwrap_or(0);
                    if bonus != 0 {
                        format!("{name:<4}{base:>4} {bonus:+}")
                    } else {
                        format!("{name:<4}{base:>4}")
                    }
                }
                _ => format!("{name:<4}   —"),
            };
            (s, theme::TEXT, true)
        }
        EquipRole::StatusCombat => {
            let s = match snap.stats.as_ref() {
                Some(st) => format!("Attack {}    Defense {}", st.attack, st.defense),
                None => "Attack —    Defense —".to_string(),
            };
            (s, theme::TEXT, true)
        }
        EquipRole::CellLabel(slot) => {
            let has_item = snap
                .equipped
                .get(slot as usize)
                .copied()
                .flatten()
                .is_some();
            // Only empty slots show the slot-name label. When an item is equipped
            // its icon fills the cell; keeping the label would sit it beside the
            // icon (row flex) and push the icon off-center.
            let color = if slot == selected_slot {
                theme::CURSOR
            } else {
                theme::MUTED
            };
            (slot.abbr().to_string(), color, !has_item)
        }
        EquipRole::DetailName => (detail_name.to_string(), theme::TITLE, true),
        EquipRole::DetailRow(i) => match detail_rows.get(i) {
            Some(line) => (line.clone(), theme::TEXT, true),
            None => (String::new(), theme::TEXT, false),
        },
        EquipRole::StorageTitle => ("Equip".to_string(), theme::TITLE, storage_active),
        EquipRole::StorageRow(i) => {
            if !storage_active {
                return (String::new(), theme::TEXT, false);
            }
            let list_idx = storage_start + i;
            if dynamic.rows.is_empty() && i == 0 {
                return ("(no equippable items)".to_string(), theme::MUTED, true);
            }
            match dynamic.rows.get(list_idx) {
                Some(row) => {
                    let cursor = list_idx == storage_cursor;
                    let equipped = match row.action {
                        crate::hud::menu::DynamicMenuAction::EquipItem { item_no, .. } => {
                            snap.equipped.get(selected_slot as usize).copied().flatten()
                                == Some(item_no)
                        }
                        _ => false,
                    };
                    let prefix = if cursor { "> " } else { "  " };
                    let suffix = if equipped { " (E)" } else { "" };
                    let color = if cursor {
                        theme::CURSOR
                    } else if equipped {
                        theme::TITLE
                    } else {
                        theme::TEXT
                    };
                    (format!("{prefix}{}{suffix}", row.label), color, true)
                }
                None => (String::new(), theme::TEXT, false),
            }
        }
    }
}

fn profile_line(
    snap: &ffxi_viewer_wire::SceneSnapshot,
    me: Option<&ffxi_viewer_wire::PartyMember>,
) -> String {
    let name = me
        .and_then(|m| m.name.clone())
        .or_else(|| snap.char_name.clone())
        .unwrap_or_else(|| "—".to_string());
    match me {
        Some(m) => {
            let main = status_panel::job_abbrev(m.main_job);
            if m.sub_job != 0 {
                let sub = status_panel::job_abbrev(m.sub_job);
                format!("{name}  {main}{} / {sub}{}", m.main_job_lv, m.sub_job_lv)
            } else {
                format!("{name}  {main}{}", m.main_job_lv)
            }
        }
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_covers_all_16_slots_once() {
        let mut seen = [false; 16];
        for row in EQUIP_GRID.iter() {
            for &slot in row.iter() {
                assert!(!seen[slot as usize], "slot {slot:?} appears twice");
                seen[slot as usize] = true;
            }
        }
        assert!(seen.iter().all(|&s| s), "every slot present");
    }

    #[test]
    fn all_is_discriminant_ordered() {
        for (i, &slot) in EquipmentIndex::ALL.iter().enumerate() {
            assert_eq!(slot as usize, i, "ALL must be in discriminant order");
            assert_eq!(EquipmentIndex::from_index(i as u8), Some(slot));
        }
        assert_eq!(EquipmentIndex::from_index(16), None);
    }

    #[test]
    fn grid_rows_are_retail_groups() {
        use EquipmentIndex::*;
        // Row 0 = weapons (Main/Sub/Ranged/Ammo); column 0 = Main/Head/Body/Back.
        assert_eq!(EQUIP_GRID[0], [Main, Sub, Range, Ammo]);
        assert_eq!(
            [
                EQUIP_GRID[0][0],
                EQUIP_GRID[1][0],
                EQUIP_GRID[2][0],
                EQUIP_GRID[3][0]
            ],
            [Main, Head, Body, Back]
        );
    }

    #[test]
    fn grid_move_wraps_and_steps() {
        // From Main (0): right -> Sub (1); down -> Head (4).
        assert_eq!(grid_move(0, 1, 0), 1);
        assert_eq!(grid_move(0, 0, 1), 4);
        // Wrap: up from Main (top of column 0) -> Back (bottom, slot 15).
        assert_eq!(grid_move(0, 0, -1), 15);
        // Wrap: left from Main (row 0) -> Ammo (col 3, row 0, slot 3).
        assert_eq!(grid_move(0, -1, 0), 3);
    }

    #[test]
    fn slot_names_and_abbr_aligned() {
        assert_eq!(SLOT_NAMES.len(), 16);
        assert_eq!(SLOT_ABBR.len(), 16);
        assert_eq!(SLOT_NAMES[10], "Waist");
        assert_eq!(SLOT_ABBR[10], "Wst");
    }
}
