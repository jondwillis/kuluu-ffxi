//! Top-center "current target" HUD panel.
//!
//! Reads `Target.id`, looks up the matching `Entity` in
//! `SceneState.snapshot.entities`, and surfaces a compact frame:
//!
//! ```text
//! ┌─────────────────────────────┐
//! │ Mandragora     [mob]        │
//! │ HP: ████░░░░░░ 42%   d=12y  │
//! └─────────────────────────────┘
//! ```
//!
//! Visibility: `Display::None` when no target is selected (or the selected
//! id is missing from the latest snapshot). Toggling display avoids
//! spawn/despawn churn at 60 Hz.
//!
//! Distance is computed in FFXI yalms, not Bevy units, by subtracting in
//! wire-space (`snapshot.self_pos.pos` and `entity.pos`). 1 yalm ≈ the
//! native unit of FFXI's coord system; the `ffxi_to_bevy` swap that the
//! 3D scene uses is irrelevant to a scalar distance.
//!
//! HP bar coloring follows the same green/yellow/red bands as classic
//! FFXI target frames so the operator's at-a-glance read matches client
//! muscle memory.

use bevy::prelude::*;
use ffxi_viewer_wire::{Entity as WireEntity, EntityKind};

use crate::hud::palette;
use crate::scene::Target;
use crate::snapshot::SceneState;

/// Marker on the panel root.
#[derive(Component)]
pub struct TargetPanel;

/// Marker on the row that carries name + kind tag.
#[derive(Component)]
pub struct TargetHeader;

/// Marker on the HP bar fill rect (width-driven).
#[derive(Component)]
pub struct TargetHpFill;

/// Marker on the HP-percent text node.
#[derive(Component)]
pub struct TargetHpText;

/// Marker on the distance-readout text node.
#[derive(Component)]
pub struct TargetDistText;

const PANEL_WIDTH_PX: f32 = 280.0;
const HP_BAR_WIDTH_PX: f32 = 200.0;
const HP_BAR_HEIGHT_PX: f32 = 6.0;

pub fn spawn_target_panel(mut commands: Commands) {
    commands
        .spawn((
            TargetPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(36.0), // below the stage bar
                // Center horizontally: left = 50% then translate by half width.
                // Bevy UI doesn't have a built-in "anchor center", so we
                // split it into a percentage offset + a fixed pixel pull.
                left: Val::Percent(50.0),
                margin: UiRect {
                    left: Val::Px(-PANEL_WIDTH_PX / 2.0),
                    ..default()
                },
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            // Header row: name (left) + kind tag (right).
            p.spawn((
                TargetHeader,
                Text::new(""),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));

            // HP bar row: track + fill + percent + distance.
            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                ..default()
            })
            .with_children(|row| {
                // HP track.
                row.spawn((
                    Node {
                        width: Val::Px(HP_BAR_WIDTH_PX),
                        height: Val::Px(HP_BAR_HEIGHT_PX),
                        ..default()
                    },
                    BackgroundColor(palette::DARK),
                ))
                .with_children(|track| {
                    track.spawn((
                        TargetHpFill,
                        Node {
                            width: Val::Px(0.0),
                            height: Val::Px(HP_BAR_HEIGHT_PX),
                            ..default()
                        },
                        BackgroundColor(hp_color(100)),
                    ));
                });
                row.spawn((
                    TargetHpText,
                    Text::new("—"),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
                row.spawn((
                    TargetDistText,
                    Text::new(""),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
            });
        });
}

/// Per-frame: hide when no target, otherwise rewrite text + bar fill.
pub fn update_target_panel_system(
    target: Res<Target>,
    state: Res<SceneState>,
    mut panel_q: Query<&mut Node, (With<TargetPanel>, Without<TargetHpFill>)>,
    mut header_q: Query<&mut Text, (With<TargetHeader>, Without<TargetHpText>, Without<TargetDistText>)>,
    mut hp_fill_q: Query<
        (&mut Node, &mut BackgroundColor),
        (With<TargetHpFill>, Without<TargetPanel>),
    >,
    mut hp_text_q: Query<
        (&mut Text, &mut TextColor),
        (With<TargetHpText>, Without<TargetHeader>, Without<TargetDistText>),
    >,
    mut dist_q: Query<&mut Text, (With<TargetDistText>, Without<TargetHeader>, Without<TargetHpText>)>,
) {
    if !state.is_changed() && !target.is_changed() {
        return;
    }

    let Ok(mut panel_node) = panel_q.single_mut() else {
        return;
    };

    let Some(target_id) = target.id else {
        if panel_node.display != Display::None {
            panel_node.display = Display::None;
        }
        return;
    };

    let snap = &state.snapshot;
    let Some(ent) = snap.entities.iter().find(|e| e.id == target_id) else {
        // Target id no longer in snapshot (despawned or out of range).
        if panel_node.display != Display::None {
            panel_node.display = Display::None;
        }
        return;
    };

    if panel_node.display == Display::None {
        panel_node.display = Display::Flex;
    }

    if let Ok(mut text) = header_q.single_mut() {
        let want = format_header(ent);
        if **text != want {
            **text = want;
        }
    }

    let pct = ent.hp_pct.unwrap_or(0);
    if let Ok((mut fill, mut bg)) = hp_fill_q.single_mut() {
        let want_w = HP_BAR_WIDTH_PX * (pct as f32 / 100.0).clamp(0.0, 1.0);
        if fill.width != Val::Px(want_w) {
            fill.width = Val::Px(want_w);
        }
        let want_color = hp_color(pct);
        if bg.0 != want_color {
            bg.0 = want_color;
        }
    }

    if let Ok((mut text, _tc)) = hp_text_q.single_mut() {
        let want = match ent.hp_pct {
            Some(p) => format!("{p}%"),
            None => "—".into(),
        };
        if **text != want {
            **text = want;
        }
    }

    if let Ok(mut text) = dist_q.single_mut() {
        let dx = snap.self_pos.pos.x - ent.pos.x;
        let dy = snap.self_pos.pos.y - ent.pos.y;
        let dz = snap.self_pos.pos.z - ent.pos.z;
        let d = (dx * dx + dy * dy + dz * dz).sqrt();
        let want = format!("d={d:.1}y");
        if **text != want {
            **text = want;
        }
    }
}

fn format_header(ent: &WireEntity) -> String {
    let name = ent.name.as_deref().unwrap_or("?");
    format!("{name}    [{}]", kind_tag(ent.kind))
}

fn kind_tag(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Pc => "pc",
        EntityKind::Npc => "npc",
        EntityKind::Mob => "mob",
        EntityKind::Pet => "pet",
        EntityKind::Other => "obj",
    }
}

fn hp_color(pct: u8) -> Color {
    match pct {
        76..=100 => Color::srgb(0.30, 0.85, 0.30), // green
        26..=75 => Color::srgb(0.95, 0.75, 0.20),  // yellow
        _ => Color::srgb(0.95, 0.20, 0.20),        // red
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::Vec3;

    fn ent(name: &str, kind: EntityKind, hp: Option<u8>, x: f32) -> WireEntity {
        WireEntity {
            id: 1,
            act_index: 0,
            kind,
            name: Some(name.into()),
            pos: Vec3 { x, y: 0.0, z: 0.0 },
            heading: 0,
            hp_pct: hp,
            bt_target_id: 0,
            claim_id: 0,
        }
    }

    #[test]
    fn header_format_includes_kind() {
        assert_eq!(format_header(&ent("Mandy", EntityKind::Mob, Some(50), 0.0)), "Mandy    [mob]");
        assert_eq!(format_header(&ent("Selh", EntityKind::Npc, None, 0.0)), "Selh    [npc]");
    }

    #[test]
    fn hp_bands() {
        assert_eq!(hp_color(100), Color::srgb(0.30, 0.85, 0.30));
        assert_eq!(hp_color(50), Color::srgb(0.95, 0.75, 0.20));
        assert_eq!(hp_color(10), Color::srgb(0.95, 0.20, 0.20));
    }
}
