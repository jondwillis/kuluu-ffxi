use bevy::prelude::*;
use ffxi_viewer_wire::{Entity as WireEntity, EntityKind, ReactorGoal};

use crate::hud::palette;
use crate::scene::Target;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct TargetPanel;

#[derive(Component)]
pub struct TargetHeader;

#[derive(Component)]
pub struct TargetHpFill;

#[derive(Component)]
pub struct TargetHpText;

#[derive(Component)]
pub struct TargetDistText;

#[derive(Component)]
pub struct TargetEngagedBadge;

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct SwingPulse {
    pub last_swing_secs: f32,
    pub prev_battle_count: usize,
}

const PULSE_DECAY_SECS: f32 = 0.25;

const PANEL_WIDTH_PX: f32 = 220.0;

const HP_BAR_WIDTH_PX: f32 = 100.0;
const HP_BAR_HEIGHT_PX: f32 = 6.0;

pub fn spawn_target_panel(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            TargetPanel,
            Node {
                position_type: PositionType::Absolute,

                bottom: Val::Px(28.0 + 90.0 + 8.0),
                right: Val::Px(8.0),
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
            p.spawn((
                TargetHeader,
                Text::new(""),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));

            p.spawn((
                TargetEngagedBadge,
                Text::new(""),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(palette::STAGE_BAD),
            ));

            p.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                ..default()
            })
            .with_children(|row| {
                row.spawn((
                    Node {
                        width: Val::Px(HP_BAR_WIDTH_PX),
                        height: Val::Px(HP_BAR_HEIGHT_PX),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    BackgroundColor(palette::DARK),
                ))
                .with_children(|track| {
                    track.spawn((
                        TargetHpFill,
                        Node {
                            width: Val::Percent(0.0),
                            height: Val::Px(HP_BAR_HEIGHT_PX),
                            flex_shrink: 0.0,
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

pub fn update_target_panel_system(
    target: Res<Target>,
    state: Res<SceneState>,
    mut panel_q: Query<(&mut Node, &mut BorderColor), (With<TargetPanel>, Without<TargetHpFill>)>,
    mut header_q: Query<
        &mut Text,
        (
            With<TargetHeader>,
            Without<TargetHpText>,
            Without<TargetDistText>,
            Without<TargetEngagedBadge>,
        ),
    >,
    mut hp_fill_q: Query<
        (&mut Node, &mut BackgroundColor),
        (With<TargetHpFill>, Without<TargetPanel>),
    >,
    mut hp_text_q: Query<
        (&mut Text, &mut TextColor),
        (
            With<TargetHpText>,
            Without<TargetHeader>,
            Without<TargetDistText>,
            Without<TargetEngagedBadge>,
        ),
    >,
    mut dist_q: Query<
        &mut Text,
        (
            With<TargetDistText>,
            Without<TargetHeader>,
            Without<TargetHpText>,
            Without<TargetEngagedBadge>,
        ),
    >,
    mut engaged_q: Query<
        &mut Text,
        (
            With<TargetEngagedBadge>,
            Without<TargetHeader>,
            Without<TargetHpText>,
            Without<TargetDistText>,
        ),
    >,
) {
    if !state.is_changed() && !target.is_changed() {
        return;
    }

    let Ok((mut panel_node, mut panel_border)) = panel_q.single_mut() else {
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
        if panel_node.display != Display::None {
            panel_node.display = Display::None;
        }
        return;
    };

    if panel_node.display == Display::None {
        panel_node.display = Display::Flex;
    }

    let party_len = snap.party.len();
    let roster_h = if party_len > 1 {
        6.0 + 40.0 * party_len as f32 + 8.0
    } else {
        0.0
    };
    let want_bottom = Val::Px(28.0 + 90.0 + 8.0 + roster_h);
    if panel_node.bottom != want_bottom {
        panel_node.bottom = want_bottom;
    }

    let self_engaged_on_target = snap
        .self_char_id
        .and_then(|sid| snap.entities.iter().find(|e| e.id == sid))
        .map(|self_pc| self_pc.bt_target_id == target_id)
        .unwrap_or(false);
    let goal_engaged_on_target = matches!(
        snap.current_goal,
        Some(ReactorGoal::Engaged { target_id: g, .. }) if g == target_id
    );
    let engaged_on_this = self_engaged_on_target || goal_engaged_on_target;
    let want_border = if engaged_on_this {
        palette::STAGE_BAD
    } else {
        palette::ACCENT
    };
    if panel_border.left != want_border {
        *panel_border = BorderColor::all(want_border);
    }
    if let Ok(mut text) = engaged_q.single_mut() {
        let want = if engaged_on_this { "⚔ Engaged" } else { "" };
        if text.as_str() != want {
            **text = want.to_string();
        }
    }

    if let Ok(mut text) = header_q.single_mut() {
        let want = format_header(ent);
        if **text != want {
            **text = want;
        }
    }

    let pct = ent.hp_pct.unwrap_or(0);
    if let Ok((mut fill, mut bg)) = hp_fill_q.single_mut() {
        let want_w = Val::Percent((pct as f32).clamp(0.0, 100.0));
        if fill.width != want_w {
            fill.width = want_w;
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

pub fn detect_swing_pulse_system(
    time: Res<Time>,
    state: Res<crate::snapshot::SceneState>,
    mut pulse: ResMut<SwingPulse>,
) {
    let count = crate::snapshot::rendered_chat(&state)
        .iter()
        .filter(|l| l.channel == ffxi_viewer_wire::ChatChannel::Battle)
        .count();
    if count > pulse.prev_battle_count {
        pulse.last_swing_secs = time.elapsed_secs();
    }
    pulse.prev_battle_count = count;
}

pub fn pulse_engaged_badge_color_system(
    time: Res<Time>,
    pulse: Res<SwingPulse>,
    mut q: Query<&mut TextColor, With<TargetEngagedBadge>>,
) {
    let Ok(mut tc) = q.single_mut() else {
        return;
    };
    let elapsed = (time.elapsed_secs() - pulse.last_swing_secs).max(0.0);
    let t = (elapsed / PULSE_DECAY_SECS).clamp(0.0, 1.0);

    let base = palette::STAGE_BAD.to_srgba();
    let flash = bevy::prelude::Color::WHITE.to_srgba();
    let r = flash.red + (base.red - flash.red) * t;
    let g = flash.green + (base.green - flash.green) * t;
    let b = flash.blue + (base.blue - flash.blue) * t;
    let want = bevy::prelude::Color::srgb(r, g, b);
    if tc.0 != want {
        tc.0 = want;
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
        76..=100 => Color::srgb(0.30, 0.85, 0.30),
        26..=75 => Color::srgb(0.95, 0.75, 0.20),
        _ => Color::srgb(0.95, 0.20, 0.20),
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
            face_target: 0,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            status: 0,
        }
    }

    #[test]
    fn header_format_includes_kind() {
        assert_eq!(
            format_header(&ent("Mandy", EntityKind::Mob, Some(50), 0.0)),
            "Mandy    [mob]"
        );
        assert_eq!(
            format_header(&ent("Selh", EntityKind::Npc, None, 0.0)),
            "Selh    [npc]"
        );
    }

    #[test]
    fn hp_bands() {
        assert_eq!(hp_color(100), Color::srgb(0.30, 0.85, 0.30));
        assert_eq!(hp_color(50), Color::srgb(0.95, 0.75, 0.20));
        assert_eq!(hp_color(10), Color::srgb(0.95, 0.20, 0.20));
    }

    #[test]
    fn swing_pulse_default_starts_at_zero() {
        let p = SwingPulse::default();
        assert_eq!(p.last_swing_secs, 0.0);
        assert_eq!(p.prev_battle_count, 0);
    }

    fn step_pulse(pulse: &mut SwingPulse, count: usize, now: f32) {
        if count > pulse.prev_battle_count {
            pulse.last_swing_secs = now;
        }
        pulse.prev_battle_count = count;
    }

    #[test]
    fn swing_pulse_latches_on_battle_line_growth() {
        let mut p = SwingPulse::default();

        step_pulse(&mut p, 0, 1.0);
        assert_eq!(p.last_swing_secs, 0.0);

        step_pulse(&mut p, 1, 2.0);
        assert_eq!(p.last_swing_secs, 2.0);

        step_pulse(&mut p, 1, 2.5);
        assert_eq!(p.last_swing_secs, 2.0);

        step_pulse(&mut p, 3, 3.0);
        assert_eq!(p.last_swing_secs, 3.0);
    }

    #[test]
    fn swing_pulse_does_not_latch_on_count_shrink() {
        let mut p = SwingPulse {
            last_swing_secs: 5.0,
            prev_battle_count: 256,
        };
        step_pulse(&mut p, 200, 10.0);
        assert_eq!(p.last_swing_secs, 5.0, "shrink must not latch");
        assert_eq!(p.prev_battle_count, 200);
    }
}
