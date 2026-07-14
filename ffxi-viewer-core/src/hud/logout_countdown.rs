use bevy::prelude::*;
use ffxi_viewer_wire::SceneSnapshot;

use crate::hud::palette;
use crate::snapshot::SceneState;

fn blocker_diagnostic(snap: &SceneSnapshot) -> String {
    if let Some(d) = &snap.dialog {
        let npc = d
            .npc_name
            .clone()
            .unwrap_or_else(|| format!("#{:08X}", d.npc_id));
        return format!(
            "Active dialog detected (NPC: {npc}, event_id={}, mode={}). \
             Close the NPC menu/dialog before retrying.",
            d.event_id, d.mode
        );
    }
    if !snap.status_icons.is_empty() {
        return format!(
            "Active status icons: {:?}. One of these is likely an \
             AbnormalStatus blocker (Weakness, Sleep, Charm, Petrify, \
             Encumbrance, etc.). Wait for the relevant effect to wear off.",
            snap.status_icons
        );
    }
    "No dialog or status icons visible to the client — likely Crafting \
     (synthesis in progress) or a PreventAction debuff."
        .into()
}

const OPTIMISTIC_ACK_TIMEOUT_SECS: f64 = 2.0;

const BLOCKED_DISPLAY_SECS: f64 = 5.0;

const OPTIMISTIC_TOTAL_SECS: u16 = 30;

#[derive(Message, Debug, Clone, Copy)]
pub struct LogoutRequested {
    pub shutdown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum OptimisticState {
    #[default]
    None,
    Optimistic {
        started_at: f64,
        shutdown: bool,
    },
    Blocked {
        entered_at: f64,
        shutdown: bool,
    },
}

#[derive(Resource, Default, Debug)]
pub struct OptimisticLogoutCountdown {
    pub state: OptimisticState,
}

#[derive(Resource, Default, Debug)]
pub struct LogoutCountdownAnchor {
    pub server_seconds: Option<u16>,
    pub shutdown: bool,
    pub anchor_secs: f64,
}

#[derive(Component)]
pub struct LogoutCountdownBanner;

#[derive(Component)]
pub struct LogoutCountdownLabel;

pub fn spawn_logout_countdown(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            LogoutCountdownBanner,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(35.0),
                left: Val::Percent(50.0),
                margin: UiRect {
                    left: Val::Px(-140.0),
                    ..default()
                },
                width: Val::Px(280.0),
                padding: UiRect::axes(Val::Px(16.0), Val::Px(10.0)),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::STAGE_BAD),
        ))
        .with_children(|p| {
            p.spawn((
                LogoutCountdownLabel,
                Text::new(""),
                TextFont {
                    font_size: 22.0.into(),
                    ..default()
                },
                TextColor(palette::STAGE_BAD),
            ));
        });
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayMode {
    Hidden,
    Counting { seconds: u32, shutdown: bool },
    LoggingOut { shutdown: bool },
    Blocked { shutdown: bool },
}

#[allow(clippy::too_many_arguments)]
fn compute_display(
    now: f64,
    server: Option<(u16, bool, f64)>,
    optimistic: OptimisticState,
) -> DisplayMode {
    if let Some((server_secs, shutdown, anchor)) = server {
        let elapsed = (now - anchor).max(0.0);
        let remaining = (server_secs as f64 - elapsed).max(0.0);
        let secs = remaining.round() as u32;
        return if secs == 0 {
            DisplayMode::LoggingOut { shutdown }
        } else {
            DisplayMode::Counting {
                seconds: secs,
                shutdown,
            }
        };
    }

    match optimistic {
        OptimisticState::None => DisplayMode::Hidden,
        OptimisticState::Optimistic {
            started_at,
            shutdown,
        } => {
            let elapsed = (now - started_at).max(0.0);
            let remaining = (OPTIMISTIC_TOTAL_SECS as f64 - elapsed).max(0.0);
            let secs = remaining.round() as u32;
            if secs == 0 {
                DisplayMode::LoggingOut { shutdown }
            } else {
                DisplayMode::Counting {
                    seconds: secs,
                    shutdown,
                }
            }
        }
        OptimisticState::Blocked {
            entered_at,
            shutdown,
        } => {
            if now - entered_at > BLOCKED_DISPLAY_SECS {
                DisplayMode::Hidden
            } else {
                DisplayMode::Blocked { shutdown }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn update_logout_countdown(
    mut requests: MessageReader<LogoutRequested>,
    time: Res<Time>,
    mut anchor: ResMut<LogoutCountdownAnchor>,
    mut optimistic: ResMut<OptimisticLogoutCountdown>,

    scene_state: Res<SceneState>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    mut banner_q: Query<&mut Node, With<LogoutCountdownBanner>>,
    mut label_q: Query<&mut Text, With<LogoutCountdownLabel>>,
) {
    let now = time.elapsed_secs_f64();

    let mut latest_request: Option<LogoutRequested> = None;
    for ev in requests.read() {
        latest_request = Some(*ev);
    }
    if let Some(req) = latest_request {
        optimistic.state = OptimisticState::Optimistic {
            started_at: now,
            shutdown: req.shutdown,
        };
    }

    let countdown = scene_state.snapshot.logout_countdown;
    match countdown {
        Some(c) if anchor.server_seconds != Some(c.seconds_remaining) => {
            anchor.server_seconds = Some(c.seconds_remaining);
            anchor.shutdown = c.shutdown;
            anchor.anchor_secs = now;

            if matches!(optimistic.state, OptimisticState::Optimistic { .. }) {
                optimistic.state = OptimisticState::None;
            }
        }
        None => {
            anchor.server_seconds = None;
        }
        _ => {}
    }

    if let OptimisticState::Optimistic {
        started_at,
        shutdown,
    } = optimistic.state
    {
        if now - started_at >= OPTIMISTIC_ACK_TIMEOUT_SECS {
            optimistic.state = OptimisticState::Blocked {
                entered_at: now,
                shutdown,
            };
            let diagnostic = blocker_diagnostic(&scene_state.snapshot);
            let label = if shutdown { "/shutdown" } else { "/logout" };
            toasts.write(crate::snapshot::ToastEvent::debug(format!(
                "{label}: server did not acknowledge (silent reject \
                 by 0x0e7_reqlogout.cpp validator). {diagnostic}"
            )));
        }
    }

    let server_anchor = anchor
        .server_seconds
        .map(|s| (s, anchor.shutdown, anchor.anchor_secs));
    let mode = compute_display(now, server_anchor, optimistic.state);

    let Ok(mut node) = banner_q.single_mut() else {
        return;
    };
    let Ok(mut text) = label_q.single_mut() else {
        return;
    };

    let (display_flex, label) = match mode {
        DisplayMode::Hidden => (false, String::new()),
        DisplayMode::Counting { seconds, shutdown } => (
            true,
            if shutdown {
                format!("Shutdown in {seconds}s")
            } else {
                format!("Logout in {seconds}s")
            },
        ),
        DisplayMode::LoggingOut { shutdown } => (
            true,
            if shutdown {
                "Shutting down…".to_string()
            } else {
                "Logging out…".to_string()
            },
        ),
        DisplayMode::Blocked { shutdown } => (
            true,
            if shutdown {
                "Shutdown blocked".to_string()
            } else {
                "Logout blocked".to_string()
            },
        ),
    };

    let want_display = if display_flex {
        Display::Flex
    } else {
        Display::None
    };
    if node.display != want_display {
        node.display = want_display;
    }
    if display_flex && **text != label {
        **text = label;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_when_nothing_pending() {
        let mode = compute_display(100.0, None, OptimisticState::None);
        assert_eq!(mode, DisplayMode::Hidden);
    }

    #[test]
    fn server_wins_over_optimistic() {
        let server = Some((25u16, false, 100.0));
        let opt = OptimisticState::Optimistic {
            started_at: 100.0,
            shutdown: false,
        };
        let mode = compute_display(100.5, server, opt);
        assert!(matches!(
            mode,
            DisplayMode::Counting {
                seconds: 24 | 25,
                shutdown: false
            }
        ));
    }

    #[test]
    fn optimistic_counts_down_smoothly() {
        let opt = OptimisticState::Optimistic {
            started_at: 0.0,
            shutdown: false,
        };

        let mode = compute_display(5.0, None, opt);
        assert_eq!(
            mode,
            DisplayMode::Counting {
                seconds: 25,
                shutdown: false
            }
        );

        let mode = compute_display(2.5, None, opt);
        assert!(matches!(
            mode,
            DisplayMode::Counting {
                seconds: 27 | 28,
                ..
            }
        ));
    }

    #[test]
    fn optimistic_at_zero_shows_logging_out() {
        let opt = OptimisticState::Optimistic {
            started_at: 0.0,
            shutdown: false,
        };
        let mode = compute_display(OPTIMISTIC_TOTAL_SECS as f64 + 0.1, None, opt);
        assert_eq!(mode, DisplayMode::LoggingOut { shutdown: false });
    }

    #[test]
    fn blocked_displays_then_hides() {
        let blocked = OptimisticState::Blocked {
            entered_at: 0.0,
            shutdown: false,
        };

        assert_eq!(
            compute_display(2.0, None, blocked),
            DisplayMode::Blocked { shutdown: false }
        );

        assert_eq!(
            compute_display(BLOCKED_DISPLAY_SECS + 0.5, None, blocked),
            DisplayMode::Hidden
        );
    }

    #[test]
    fn shutdown_label_propagates() {
        let opt = OptimisticState::Optimistic {
            started_at: 0.0,
            shutdown: true,
        };
        let mode = compute_display(5.0, None, opt);
        assert_eq!(
            mode,
            DisplayMode::Counting {
                seconds: 25,
                shutdown: true
            }
        );
    }
}
