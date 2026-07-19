use bevy::prelude::*;

use crate::hud::item_meta::{compose_item_detail, ItemDetail};
use crate::hud::style::{self, theme};
use crate::snapshot::SceneState;

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct CheckTarget {
    pub open: bool,
    pub target_id: Option<u32>,
}

pub const CHECK_GRID_SLOTS: &[(u8, &str)] = &[
    (0, "Main"),
    (1, "Sub"),
    (2, "Range"),
    (3, "Ammo"),
    (4, "Head"),
    (9, "Neck"),
    (11, "Ear1"),
    (12, "Ear2"),
    (5, "Body"),
    (6, "Hands"),
    (13, "Ring1"),
    (14, "Ring2"),
    (15, "Back"),
    (10, "Waist"),
    (7, "Legs"),
    (8, "Feet"),
];

const PANEL_WIDTH_PX: f32 = 320.0;

#[derive(Component)]
pub struct CheckView;

#[derive(Component)]
pub struct CheckWaresSection;

#[derive(Component)]
pub struct CheckWaresRow {
    pub idx: usize,
}

#[derive(Component)]
pub struct CheckGridCell {
    pub grid_index: usize,
}

#[derive(Component)]
pub struct CheckJobRibbon;

const MAX_WARES_ROWS: usize = 8;

pub fn spawn_check_view(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            CheckView,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(20.0),
                left: Val::Percent(30.0),
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("Check"),
                style::text_font(14.0),
                TextColor(theme::TITLE),
            ));

            p.spawn((
                CheckWaresSection,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(1.0),
                    display: Display::None,
                    ..default()
                },
            ))
            .with_children(|w| {
                w.spawn((
                    Text::new("View Wares"),
                    style::text_font(13.0),
                    TextColor(theme::MUTED),
                ));
                for idx in 0..MAX_WARES_ROWS {
                    w.spawn((
                        CheckWaresRow { idx },
                        Text::new(""),
                        style::text_font(13.0),
                        TextColor(theme::TEXT),
                    ));
                }
            });

            p.spawn((Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(1.0),
                ..default()
            },))
                .with_children(|g| {
                    for grid_index in 0..CHECK_GRID_SLOTS.len() {
                        g.spawn((
                            CheckGridCell { grid_index },
                            Text::new(""),
                            style::text_font(13.0),
                            TextColor(theme::TEXT),
                        ));
                    }
                });

            p.spawn((
                CheckJobRibbon,
                Text::new(""),
                style::text_font(13.0),
                TextColor(theme::TITLE),
            ));
        });
}

pub fn update_check_view(
    target: Res<CheckTarget>,
    state: Res<SceneState>,
    mut view_q: Query<&mut Node, With<CheckView>>,
    mut wares_section_q: Query<&mut Node, (With<CheckWaresSection>, Without<CheckView>)>,
    mut wares_row_q: Query<(&CheckWaresRow, &mut Text), Without<CheckGridCell>>,
    mut grid_q: Query<(&CheckGridCell, &mut Text, &mut TextColor), Without<CheckWaresRow>>,
    mut ribbon_q: Query<
        &mut Text,
        (
            With<CheckJobRibbon>,
            Without<CheckWaresRow>,
            Without<CheckGridCell>,
        ),
    >,
) {
    let Ok(mut view_node) = view_q.single_mut() else {
        return;
    };

    if !target.open {
        if view_node.display != Display::None {
            view_node.display = Display::None;
        }
        return;
    }
    if view_node.display == Display::None {
        view_node.display = Display::Flex;
    }

    let snap = &state.snapshot;
    let check = snap
        .check
        .as_ref()
        .filter(|c| target.target_id == Some(c.target_id));

    let bazaar = &snap.bazaar;
    if let Ok(mut wares_node) = wares_section_q.single_mut() {
        let want_display = if bazaar.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        if wares_node.display != want_display {
            wares_node.display = want_display;
        }
    }
    for (row, mut text) in wares_row_q.iter_mut() {
        let want = match bazaar.get(row.idx) {
            Some(entry) => {
                let name = ffxi_proto::item_names::lookup(entry.item_no)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("item #{}", entry.item_no));
                if entry.quantity > 1 {
                    format!("  {name} x{}  {} gil", entry.quantity, entry.price)
                } else {
                    format!("  {name}  {} gil", entry.price)
                }
            }
            None => String::new(),
        };
        if **text != want {
            **text = want;
        }
    }

    for (cell, mut text, mut color) in grid_q.iter_mut() {
        let Some(&(slot_id, slot_label)) = CHECK_GRID_SLOTS.get(cell.grid_index) else {
            continue;
        };
        let item_no = check.and_then(|c| c.equipped.get(slot_id as usize).copied().flatten());
        let (body, filled) = match item_no {
            Some(no) => {
                let detail: ItemDetail = compose_item_detail(no, snap, None);
                let name = item_label(no, &detail);
                (format!("{slot_label:<6}: {name}"), true)
            }
            None => (format!("{slot_label:<6}: —"), false),
        };
        if **text != body {
            **text = body;
        }
        let want_color = if filled { theme::TEXT } else { theme::MUTED };
        if color.0 != want_color {
            color.0 = want_color;
        }
    }

    if let Ok(mut text) = ribbon_q.single_mut() {
        let want = job_ribbon(check);
        if **text != want {
            **text = want;
        }
    }
}

fn item_label(item_no: u16, detail: &ItemDetail) -> String {
    if let Some(s) = detail.static_.as_ref() {
        if !s.name.is_empty() {
            return s.name.clone();
        }
    }
    ffxi_proto::item_names::lookup(item_no)
        .map(str::to_string)
        .unwrap_or_else(|| format!("item #{item_no}"))
}

fn job_ribbon(check: Option<&ffxi_viewer_wire::CheckResult>) -> String {
    match check {
        Some(c) if c.main_job != 0 => {
            let job = ffxi_proto::job_names::lookup(c.main_job as u16).unwrap_or("Adventurer");
            match c.sub_job {
                0 => format!("Lv.{} {job}", c.main_job_lv),
                sub => {
                    let sub_job = ffxi_proto::job_names::lookup(sub as u16).unwrap_or("Adventurer");
                    format!("Lv.{} {job}/{sub_job}", c.main_job_lv)
                }
            }
        }
        _ => "Lv.? —".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_grid_has_sixteen_unique_slots() {
        assert_eq!(CHECK_GRID_SLOTS.len(), 16, "all 16 equipment slots present");
        let mut ids: Vec<u8> = CHECK_GRID_SLOTS.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 16, "no duplicate slot ids in the grid");
        assert_eq!(*ids.last().unwrap(), 15, "max slot id is Back (15)");
    }

    #[test]
    fn grid_reading_order_starts_main_sub() {
        assert_eq!(CHECK_GRID_SLOTS[0], (0, "Main"));
        assert_eq!(CHECK_GRID_SLOTS[1], (1, "Sub"));
        assert_eq!(CHECK_GRID_SLOTS[2], (2, "Range"));
        assert_eq!(CHECK_GRID_SLOTS[3], (3, "Ammo"));
    }
}
