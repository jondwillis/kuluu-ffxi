//! Left-anchored agent HUD card mirroring the chrome `draw_agent_hud`
//! layout from `ffxi-client/src/chrome.rs` (the TUI side). Three lines:
//!
//! ```text
//!   goal: follow #4242 d=3.5y
//!  state: [FOLLOWING]
//!  recon: 1.2s ago (520ms down)
//! ```
//!
//! - `goal` — reflects `SceneSnapshot::current_goal`, falls back to `—`.
//! - `state` — color-coded pill mirroring goal kind (cyan/green/yellow/red).
//! - `recon` — wall-clock age of the most recent supervisor `Reconnected`
//!   event, plus how long the downtime was. `—` when no reconnect yet.
//!
//! Sits below the stage bar at the top-left; doesn't capture pointer
//! events. `update_agent_hud_system` runs every frame because the recon
//! age decays continuously, even when no fresh snapshot has arrived.

use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::*;
use ffxi_viewer_wire::ReactorGoal;

use crate::hud::palette;
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct AgentHud;

#[derive(Component)]
pub struct GoalText;

#[derive(Component)]
pub struct StatePill;

#[derive(Component)]
pub struct ReconnectText;

/// Spawn the card at top-left, just below the 28px stage bar.
pub fn spawn_agent_hud(mut commands: Commands) {
    commands
        .spawn((
            AgentHud,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(36.0),
                left: Val::Px(8.0),
                width: Val::Px(260.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
        ))
        .with_children(|p| {
            p.spawn(line_row("goal:")).with_children(|row| {
                row.spawn((
                    GoalText,
                    Text::new("—".to_string()),
                    TextFont { font_size: 13.0, ..default() },
                    TextColor(palette::TEXT),
                ));
            });
            p.spawn(line_row("state:")).with_children(|row| {
                row.spawn((
                    StatePill,
                    Text::new("[IDLE]".to_string()),
                    TextFont { font_size: 13.0, ..default() },
                    TextColor(palette::MUTED),
                ));
            });
            p.spawn(line_row("recon:")).with_children(|row| {
                row.spawn((
                    ReconnectText,
                    Text::new("—".to_string()),
                    TextFont { font_size: 13.0, ..default() },
                    TextColor(palette::MUTED),
                ));
            });
        });
}

fn line_row(label: &str) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(6.0),
            ..default()
        },
        children![(
            Text::new(label.to_string()),
            TextFont { font_size: 13.0, ..default() },
            TextColor(palette::MUTED),
        )],
    )
}

/// Refresh goal/pill/recon. Runs every frame because recon decays
/// continuously even when no fresh snapshot lands.
pub fn update_agent_hud_system(
    state: Res<SceneState>,
    mut q_goal: Query<&mut Text, (With<GoalText>, Without<StatePill>, Without<ReconnectText>)>,
    mut q_pill: Query<
        (&mut Text, &mut TextColor),
        (With<StatePill>, Without<GoalText>, Without<ReconnectText>),
    >,
    mut q_recon: Query<&mut Text, (With<ReconnectText>, Without<GoalText>, Without<StatePill>)>,
) {
    if let Ok(mut text) = q_goal.single_mut() {
        **text = goal_label(state.snapshot.current_goal.as_ref());
    }
    if let Ok((mut text, mut color)) = q_pill.single_mut() {
        let (label, c) = state_pill(state.snapshot.current_goal.as_ref());
        **text = label;
        color.0 = c;
    }
    if let Ok(mut text) = q_recon.single_mut() {
        **text = format_reconnect(state.snapshot.last_reconnect.as_ref(), now_unix_ms());
    }
}

fn goal_label(g: Option<&ReactorGoal>) -> String {
    match g {
        None | Some(ReactorGoal::Idle) => "—".to_string(),
        Some(ReactorGoal::Following { target_id, distance }) => {
            format!("follow #{target_id:x} d={distance:.1}y")
        }
        Some(ReactorGoal::Engaged { target_id, attack_issued }) => {
            let suffix = if *attack_issued { " (atk sent)" } else { "" };
            format!("engage #{target_id:x}{suffix}")
        }
        Some(ReactorGoal::Pathing { x, y, z, waypoints_remaining }) => {
            format!("path → ({x:.1}, {y:.1}, {z:.1}) [{waypoints_remaining} wp]")
        }
        Some(ReactorGoal::Banking { threshold, mog_house_zoneline }) => {
            format!("bank ≥{threshold} → zoneline {mog_house_zoneline}")
        }
    }
}

fn state_pill(g: Option<&ReactorGoal>) -> (String, Color) {
    match g {
        None | Some(ReactorGoal::Idle) => ("[IDLE]".to_string(), palette::MUTED),
        Some(ReactorGoal::Following { .. }) => ("[FOLLOWING]".to_string(), palette::ACCENT),
        Some(ReactorGoal::Engaged { .. }) => ("[ENGAGED]".to_string(), palette::STAGE_BAD),
        Some(ReactorGoal::Pathing { .. }) => ("[PATHING]".to_string(), palette::STAGE_TRANSITIONING),
        Some(ReactorGoal::Banking { .. }) => ("[BANKING]".to_string(), palette::STAGE_GOOD),
    }
}

fn format_reconnect(rc: Option<&ffxi_viewer_wire::ReconnectInfo>, now_ms: u64) -> String {
    let Some(rc) = rc else { return "—".to_string() };
    let age_ms = now_ms.saturating_sub(rc.at_unix_ms);
    format!("{} ago ({})", format_age_short(age_ms), format_duration_ms(rc.downtime_ms))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn format_age_short(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f32 / 1_000.0)
    } else if ms < 3_600_000 {
        format!("{}m", ms / 60_000)
    } else {
        format!("{}h", ms / 3_600_000)
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms down")
    } else {
        format!("{:.1}s down", ms as f32 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_label_engaged_includes_attack_issued() {
        let g = ReactorGoal::Engaged { target_id: 0x99, attack_issued: true };
        assert_eq!(goal_label(Some(&g)), "engage #99 (atk sent)");
    }

    #[test]
    fn goal_label_pathing_shows_waypoint_count() {
        let g = ReactorGoal::Pathing { x: 12.3, y: 0.0, z: -45.6, waypoints_remaining: 3 };
        assert_eq!(goal_label(Some(&g)), "path → (12.3, 0.0, -45.6) [3 wp]");
    }

    #[test]
    fn goal_label_idle_when_none() {
        assert_eq!(goal_label(None), "—");
        assert_eq!(goal_label(Some(&ReactorGoal::Idle)), "—");
    }

    #[test]
    fn state_pill_color_coded() {
        let (label, _) = state_pill(Some(&ReactorGoal::Engaged {
            target_id: 1, attack_issued: false,
        }));
        assert_eq!(label, "[ENGAGED]");

        let (label, _) = state_pill(None);
        assert_eq!(label, "[IDLE]");
    }

    #[test]
    fn format_reconnect_uses_wallclock_diff() {
        let rc = ffxi_viewer_wire::ReconnectInfo {
            downtime_ms: 520,
            at_unix_ms: 1_000_000,
        };
        // 1.2s after the reconnect.
        let s = format_reconnect(Some(&rc), 1_000_000 + 1_200);
        assert_eq!(s, "1.2s ago (520ms down)");
    }

    #[test]
    fn format_reconnect_dash_when_none() {
        assert_eq!(format_reconnect(None, 0), "—");
    }
}
