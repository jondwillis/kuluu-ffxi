use bevy::prelude::*;

use crate::hud::style::{self, theme};
use crate::snapshot::SceneState;

#[derive(Resource, Debug, Clone, Copy)]
pub struct NetStatusVisible(pub bool);

impl Default for NetStatusVisible {
    fn default() -> Self {
        Self(true)
    }
}

/// Fixed slot width so the menu help bar can reserve space for it exactly.
/// The readout itself draws no box - just the arrows and text (USER
/// 2026-10-28: "it should not be drawing its own box.. it should be just the
/// text/arrws"; any frame belongs to the top controls, which own their fade).
pub const NET_PANEL_W: f32 = 122.0;

/// Text sizes for the readout. Sized so the tallest stacks (the two baud rows, the arrow pair)
/// fit inside one framed menu row - `menu_help_bar::BAR_HEIGHT` minus its borders, twenty-four
/// px (USER 2026-10-28: "height should match the other bar and the inside to match fitting
/// inside"; the box is now owned by the top controls at exactly that height).
const NET_ARROW_FONT_PX: f32 = 8.0;
const NET_PERCENT_FONT_PX: f32 = 11.0;
const NET_BAUD_FONT_PX: f32 = 9.0;

/// Rhythm between the three groups (arrows | percent | S-R stack) and within a baud row. Set by
/// side-by-side with retail's readout (USER screenshot 2026-10-28: "spacing on kuluu crap.. its
/// much tigher/neater on retail"): even gaps between the groups, and the value sits close to its
/// S/R letter - wide right-aligned wells were what made kuluu's rows read loose.
const NET_COLUMN_GAP_PX: f32 = 8.0;
const NET_VALUE_GAP_PX: f32 = 3.0;

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
            // Text and arrows only: no fill, no border. The old own-frame made an
            // empty box float at the viewport corner while the top controls' framed
            // segments fade below it (USER 2026-10-28: "separate teh box from the
            // send/recieve system and align it"). Anchored off the same inset the old
            // 1px border + 6px padding produced, starting at the bar's row so it sits
            // aligned with the top controls instead of the screen edge.
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(crate::hud::menu_help_bar::BAR_TOP),
                right: Val::Px(7.0),
                width: Val::Px(NET_PANEL_W - 7.0),
                height: Val::Px(crate::hud::menu_help_bar::BAR_HEIGHT),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                column_gap: Val::Px(NET_COLUMN_GAP_PX),
                ..default()
            },
            // Text stays above the fading bar band either way.
            GlobalZIndex(style::WINDOW_Z + 1),
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
                    style::text_font(NET_ARROW_FONT_PX),
                    TextColor(health_color(100)),
                ));
                col.spawn((
                    NetRecvArrow,
                    Text::new("\u{25BC}"),
                    style::text_font(NET_ARROW_FONT_PX),
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
                    style::text_font(NET_PERCENT_FONT_PX),
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

// Well widths for the right-aligned fields, sized to their longest shown value plus a hair:
// "100%" in the percent box (eleven px glyphs), four digits in the baud box.
const PERCENT_BOX_W: f32 = 28.0;
const BAUD_BOX_W: f32 = 26.0;

fn spawn_baud_row<M: Component>(col: &mut ChildSpawnerCommands, prefix: &str, value_marker: M) {
    col.spawn(Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: Val::Px(NET_VALUE_GAP_PX),
        ..default()
    })
    .with_children(|row| {
        row.spawn((
            Text::new(prefix.to_string()),
            style::text_font(NET_BAUD_FONT_PX),
            TextColor(theme::MUTED),
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
                style::text_font(NET_BAUD_FONT_PX),
                TextColor(theme::TEXT),
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
