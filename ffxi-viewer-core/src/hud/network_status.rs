use bevy::prelude::*;

use crate::hud::palette;
use crate::snapshot::SceneState;

#[derive(Resource, Debug, Clone, Copy)]
pub struct NetStatusVisible(pub bool);

impl Default for NetStatusVisible {
    fn default() -> Self {
        Self(true)
    }
}

#[derive(Component)]
pub struct NetworkStatusPanel;

#[derive(Component)]
pub struct NetSendArrow;

#[derive(Component)]
pub struct NetRecvArrow;

#[derive(Component)]
pub struct NetPercentLabel;

#[derive(Component)]
pub struct NetSendLabel;

#[derive(Component)]
pub struct NetRecvLabel;

pub fn spawn_network_status(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            NetworkStatusPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(4.0),
                right: Val::Px(4.0),
                padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
            GlobalZIndex(10),
        ))
        .with_children(|p| {
            p.spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|col| {
                col.spawn((
                    NetSendArrow,
                    Text::new("\u{25B2}"),
                    TextFont {
                        font_size: 9.0,
                        ..default()
                    },
                    TextColor(health_color(100)),
                ));
                col.spawn((
                    NetRecvArrow,
                    Text::new("\u{25BC}"),
                    TextFont {
                        font_size: 9.0,
                        ..default()
                    },
                    TextColor(health_color(100)),
                ));
            });

            p.spawn(Node {
                width: Val::Px(PERCENT_BOX_W),
                justify_content: JustifyContent::FlexEnd,
                ..default()
            })
            .with_children(|b| {
                b.spawn((
                    NetPercentLabel,
                    Text::new("100%"),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(health_color(100)),
                ));
            });

            p.spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexStart,
                ..default()
            })
            .with_children(|col| {
                spawn_baud_row(col, "S", NetSendLabel);
                spawn_baud_row(col, "R", NetRecvLabel);
            });
        });
}

const PERCENT_BOX_W: f32 = 36.0;
const BAUD_BOX_W: f32 = 38.0;

fn spawn_baud_row<M: Component>(col: &mut ChildSpawnerCommands, prefix: &str, value_marker: M) {
    col.spawn(Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(3.0),
        ..default()
    })
    .with_children(|row| {
        row.spawn((
            Text::new(prefix.to_string()),
            TextFont {
                font_size: 11.0,
                ..default()
            },
            TextColor(palette::MUTED),
        ));
        row.spawn(Node {
            width: Val::Px(BAUD_BOX_W),
            justify_content: JustifyContent::FlexEnd,
            ..default()
        })
        .with_children(|b| {
            b.spawn((
                value_marker,
                Text::new("0"),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(palette::TEXT),
            ));
        });
    });
}

#[allow(clippy::type_complexity)]
pub fn update_network_status(
    time: Res<Time>,
    state: Res<SceneState>,
    mut send_arrow: Query<
        &mut TextColor,
        (
            With<NetSendArrow>,
            Without<NetRecvArrow>,
            Without<NetPercentLabel>,
        ),
    >,
    mut recv_arrow: Query<
        &mut TextColor,
        (
            With<NetRecvArrow>,
            Without<NetSendArrow>,
            Without<NetPercentLabel>,
        ),
    >,
    mut percent: Query<
        (&mut Text, &mut TextColor),
        (
            With<NetPercentLabel>,
            Without<NetSendArrow>,
            Without<NetRecvArrow>,
        ),
    >,
    mut send_label: Query<
        &mut Text,
        (
            With<NetSendLabel>,
            Without<NetPercentLabel>,
            Without<NetRecvLabel>,
        ),
    >,
    mut recv_label: Query<
        &mut Text,
        (
            With<NetRecvLabel>,
            Without<NetPercentLabel>,
            Without<NetSendLabel>,
        ),
    >,
) {
    let net = state.snapshot.net_stats;
    let overall = net.send_health.min(net.recv_health);

    let t = time.elapsed_secs();

    if let Ok(mut tc) = send_arrow.single_mut() {
        tc.0 = animated_arrow_color(net.send_health, t);
    }
    if let Ok(mut tc) = recv_arrow.single_mut() {
        tc.0 = animated_arrow_color(net.recv_health, t);
    }
    if let Ok((mut text, mut tc)) = percent.single_mut() {
        let want = format!("{overall}%");
        if **text != want {
            **text = want;
        }
        tc.0 = health_color(overall);
    }
    if let Ok(mut text) = send_label.single_mut() {
        let want = net.send_bps.to_string();
        if **text != want {
            **text = want;
        }
    }
    if let Ok(mut text) = recv_label.single_mut() {
        let want = net.recv_bps.to_string();
        if **text != want {
            **text = want;
        }
    }
}

pub fn apply_net_status_visibility(
    visible: Res<NetStatusVisible>,
    mut q: Query<&mut Visibility, With<NetworkStatusPanel>>,
) {
    if !visible.is_changed() {
        return;
    }
    let want = if visible.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut v in q.iter_mut() {
        if *v != want {
            *v = want;
        }
    }
}

pub fn health_color(pct: u8) -> Color {
    let good = Color::srgb(0.0, 0.85, 0.0);
    let warn = Color::srgb(1.0, 0.85, 0.0);
    let bad = Color::srgb(0.95, 0.20, 0.20);
    let f = (pct as f32 / 100.0).clamp(0.0, 1.0);
    if f >= 0.5 {
        warn.mix(&good, (f - 0.5) * 2.0)
    } else {
        bad.mix(&warn, f * 2.0)
    }
}

fn animated_arrow_color(health: u8, t: f32) -> Color {
    let base = health_color(health);
    let speed = 1.5 + (health as f32 / 100.0) * 4.5;
    let pulse = 0.5 + 0.5 * (t * speed).sin();
    let alpha = 0.45 + 0.55 * pulse;
    base.with_alpha(alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(c: Color) -> (f32, f32, f32) {
        let s = c.to_srgba();
        (s.red, s.green, s.blue)
    }

    #[test]
    fn full_health_is_green() {
        let (r, g, b) = rgb(health_color(100));
        assert!(g > 0.8 && r < 0.05 && b < 0.05);
    }

    #[test]
    fn zero_health_is_red() {
        let (r, g, b) = rgb(health_color(0));
        assert!(r > 0.8 && g < 0.25 && b < 0.25);
    }

    #[test]
    fn mid_health_is_yellowish() {
        let (r, g, b) = rgb(health_color(50));
        assert!(r > 0.8 && g > 0.8 && b < 0.05);
    }

    #[test]
    fn arrow_pulse_stays_within_alpha_band() {
        for i in 0..64 {
            let t = i as f32 * 0.1;
            let a = animated_arrow_color(100, t).to_srgba().alpha;
            assert!((0.44..=1.01).contains(&a), "alpha out of band: {a}");
        }
    }
}
