use bevy::prelude::*;
use ffxi_viewer_core::hud::{palette, DevHud, HudVerbosity};
use ffxi_viewer_core::{InGameEntity, OperatorCamera, SceneState, Target};
use ffxi_viewer_wire::EntityKind;

use super::input::{build_tab_candidates, TabCycleStack};

const MAX_ROWS: usize = 24;
const INFO_LINES: usize = 1;
const REFRESH_INTERVAL: f32 = 0.2;
const ROW_FONT: f32 = 12.0;
const NAME_WIDTH: usize = 16;

#[derive(Component)]
pub struct TargetListPanel;

#[derive(Component)]
pub struct InfoLine(pub usize);

#[derive(Component)]
pub struct TargetListRow(pub usize);

pub fn spawn_target_list_hud(mut commands: Commands) {
    commands
        .spawn((
            InGameEntity,
            DevHud,
            TargetListPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(96.0),
                right: Val::Px(8.0),
                width: Val::Px(360.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(1.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
            GlobalZIndex(10),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            for i in 0..INFO_LINES {
                p.spawn((
                    InfoLine(i),
                    Text::new(""),
                    TextFont {
                        font_size: ROW_FONT,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                    TextLayout::new_with_no_wrap(),
                ));
            }
            for i in 0..MAX_ROWS {
                p.spawn((
                    TargetListRow(i),
                    Text::new(""),
                    TextFont {
                        font_size: ROW_FONT,
                        ..default()
                    },
                    TextColor(palette::TEXT),
                    TextLayout::new_with_no_wrap(),
                    Node {
                        display: Display::None,
                        ..default()
                    },
                ));
            }
        });
}

struct RowData {
    text: String,
    color: Color,
}

pub fn update_target_list_hud(
    verbosity: Res<HudVerbosity>,
    time: Res<Time>,
    scene: Res<SceneState>,
    target: Res<Target>,
    tab_stack: Res<TabCycleStack>,
    cam_q: Query<(&Camera, &Transform), With<OperatorCamera>>,
    mut refresh: Local<f32>,
    mut info_q: Query<(&InfoLine, &mut Text, &mut TextColor), Without<TargetListRow>>,
    mut row_q: Query<(&TargetListRow, &mut Text, &mut TextColor, &mut Node), Without<InfoLine>>,
) {
    if !verbosity.dev_hud {
        return;
    }
    *refresh += time.delta_secs();
    if *refresh < REFRESH_INTERVAL {
        return;
    }
    *refresh = 0.0;

    let Ok((camera, cam_t)) = cam_q.single() else {
        return;
    };
    let cam_global = GlobalTransform::from(*cam_t);
    let snap = &scene.snapshot;

    let party_ids: Vec<u32> = snap.party.iter().map(|p| p.id).collect();
    let owner = snap.self_char_id.unwrap_or(0);
    let owned_pet_ids: Vec<u32> = snap
        .entities
        .iter()
        .filter(|e| matches!(e.kind, EntityKind::Pet) && e.claim_id == owner)
        .map(|e| e.id)
        .collect();

    let order = build_tab_candidates(
        &snap.entities,
        snap.self_pos.pos,
        snap.self_char_id,
        &party_ids,
        &owned_pet_ids,
        |world_pos| camera.world_to_ndc(&cam_global, world_pos),
    );

    let info = build_info_lines(&order, &tab_stack);
    for (line, mut text, mut color) in info_q.iter_mut() {
        if let Some((s, c)) = info.get(line.0) {
            if **text != *s {
                **text = s.clone();
            }
            color.0 = *c;
        }
    }

    let rows = build_rows(snap, &order, &party_ids, &owned_pet_ids, target.id);
    for (row, mut text, mut color, mut node) in row_q.iter_mut() {
        match rows.get(row.0) {
            Some(data) => {
                if **text != data.text {
                    **text = data.text.clone();
                }
                color.0 = data.color;
                node.display = Display::Flex;
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                    **text = String::new();
                }
            }
        }
    }
}

fn build_info_lines(order: &[u32], tab_stack: &TabCycleStack) -> Vec<(String, Color)> {
    let header = (
        format!(
            "TAB CYCLE \u{21bb}  {} on-screen   pending:{}  idle:{:.1}s",
            order.len(),
            tab_stack.pending_len(),
            tab_stack.idle_secs(),
        ),
        palette::ACCENT,
    );

    vec![header]
}

fn build_rows(
    snap: &ffxi_viewer_wire::SceneSnapshot,
    order: &[u32],
    party_ids: &[u32],
    owned_pet_ids: &[u32],
    current: Option<u32>,
) -> Vec<RowData> {
    let from = snap.self_pos.pos;
    let mut rows: Vec<RowData> = Vec::with_capacity(order.len().min(MAX_ROWS));
    for (idx, &id) in order.iter().enumerate() {
        if idx >= MAX_ROWS {
            break;
        }
        if idx == MAX_ROWS - 1 && order.len() > MAX_ROWS {
            rows.push(RowData {
                text: format!("   \u{2026} +{} more", order.len() - (MAX_ROWS - 1)),
                color: palette::MUTED,
            });
            break;
        }

        let entity = snap.entities.iter().find(|e| e.id == id);
        let name = entity
            .and_then(|e| e.name.clone())
            .unwrap_or_else(|| format!("#{id:08X}"));
        let dist = entity
            .map(|e| {
                let dx = e.pos.x - from.x;
                let dy = e.pos.y - from.y;
                let dz = e.pos.z - from.z;
                (dx * dx + dy * dy + dz * dz).sqrt()
            })
            .unwrap_or(0.0);
        let kind = entity.map(|e| e.kind).unwrap_or(EntityKind::Other);
        let is_party = party_ids.contains(&id) || owned_pet_ids.contains(&id);
        let is_current = current == Some(id);

        let marker = if is_current { '\u{25b6}' } else { '\u{2502}' };
        let tag = if is_party { " \u{2605}" } else { "" };
        let text = format!(
            "{marker}{idx:>2} {} {:>3} {dist:>4.0}y{tag}",
            truncate_pad(&name, NAME_WIDTH),
            kind_label(kind),
        );
        let color = if is_current {
            palette::ACCENT
        } else if is_party {
            palette::STAGE_GOOD
        } else {
            palette::TEXT
        };
        rows.push(RowData { text, color });
    }
    rows
}

fn kind_label(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Pc => "PC",
        EntityKind::Npc => "NPC",
        EntityKind::Mob => "MOB",
        EntityKind::Pet => "PET",
        EntityKind::Other => "—",
    }
}

fn truncate_pad(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count > width {
        let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    } else {
        format!("{s:<width$}")
    }
}
