//! `/logout` and `/shutdown` countdown widget.
//!
//! The widget runs in one of three modes per frame, in order of priority:
//!
//! 1. **Server-authoritative.** `SceneSnapshot.logout_countdown` is `Some`,
//!    meaning a 0x053 SYSTEMMES id=7/35 tick has landed. Smooth-interpolate
//!    the server seconds value via a local anchor (server messages arrive
//!    every 5s — without interpolation the number jumps by 5).
//! 2. **Optimistic client-side.** A `LogoutRequested` message was just
//!    written by the slash dispatcher but no `0x053` has confirmed yet.
//!    Show a 30s countdown ticking from the dispatch instant. This is the
//!    visual feedback the operator expects between pressing Enter and the
//!    server's first acknowledgement (typically <100ms; server fires
//!    `EFFECT_LEAVEGAME::onEffectGain → messageSystem(30)` immediately).
//! 3. **Blocked.** No `0x053` arrived within
//!    [`OPTIMISTIC_ACK_TIMEOUT_SECS`]. The 0x0E7 `ReqLogout` validator
//!    (`vendor/server/src/map/packets/c2s/0x0e7_reqlogout.cpp::validate`)
//!    silently rejects when the player is `InEvent`, `AbnormalStatus`,
//!    `Crafting`, or `PreventAction`. We surface that as an explicit
//!    "blocked" state plus a system chat toast — without this branch the
//!    user gets the original "pressed /logout, nothing happened" failure
//!    mode that prompted this widget.
//!
//! After ~5s of `Blocked` display, the widget hides itself.
//!
//! Server / optimistic state can interleave: pressing `/logout` in an MH
//! disconnects without a `0x053` (`leavegame.lua:21-27` short-circuits to
//! `target:leaveGame()`). The Disconnected snapshot stage clears
//! `snapshot.logout_countdown` and we hide promptly via the same path.

use bevy::prelude::*;
use ffxi_viewer_wire::SceneSnapshot;

use crate::hud::palette;
use crate::snapshot::SceneState;

/// Inspect the client-visible state for the most likely 0x0E7 blocker.
/// The LSB validator (`vendor/server/src/map/packets/c2s/0x0e7_reqlogout.cpp:30`)
/// rejects on `InEvent | AbnormalStatus | Crafting | PreventAction` but
/// never tells us which. The client *does* see the active dialog and the
/// status-icon list, so we can name the likely culprit from those two
/// signals. The remaining cases (Crafting, PreventAction without a
/// visible icon) fall through to a generic message.
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

/// How long the optimistic countdown waits for a server `0x053` before
/// flipping to `Blocked`. The server's `EFFECT_LEAVEGAME::onEffectGain`
/// fires the first messageSystem immediately, so even a slow round-trip
/// should produce a 0x053 within a few hundred ms. 2 seconds is generous.
const OPTIMISTIC_ACK_TIMEOUT_SECS: f64 = 2.0;

/// How long the `Blocked` banner stays on screen before auto-hiding.
const BLOCKED_DISPLAY_SECS: f64 = 5.0;

/// Total visual time of the optimistic countdown if the server never
/// responds. Server-authoritative LogoutCountdown carries its own
/// seconds value; this is only used in pure-optimistic mode.
const OPTIMISTIC_TOTAL_SECS: u16 = 30;

/// Fired by the slash dispatcher (`view_native/text_input.rs::apply_slash_outcome`)
/// the moment a `/logout` or `/shutdown` arming variant is dispatched.
/// Listened to by [`update_logout_countdown`] to seed
/// [`OptimisticLogoutCountdown`].
#[derive(Message, Debug, Clone, Copy)]
pub struct LogoutRequested {
    pub shutdown: bool,
}

/// Visual state of the widget. `None` is the resting state — widget
/// hidden. `Optimistic` is "we dispatched 0x0E7, waiting for ack."
/// `Blocked` is "validator silently rejected; display the bad-news
/// banner for a few seconds then dismiss."
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

/// Local interpolation anchor for *server-authoritative* mode (mode 1
/// in the file-level doc). `server_seconds` is the last reported value
/// from the snapshot; `anchor_secs` is the Bevy elapsed-time we saw it.
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
    // -----------------------------------------------------------------
    // TODO(you): style block. Five-to-ten lines that define how this
    // widget *looks*. The data plumbing is fully wired — at runtime the
    // node toggles between `Display::None` (no logout pending) and
    // `Display::Flex` (countdown active or blocked), and the label
    // string is rewritten every frame.
    //
    // Pick:
    //   - position (top/left/right offsets, or anchored elsewhere)
    //   - size (Val::Px / Val::Percent)
    //   - colours — palette exposes: BACKGROUND, BORDER, ACCENT, TEXT,
    //     MUTED, DARK, STAGE_GOOD, STAGE_TRANSITIONING, STAGE_BAD
    //   - font_size for the label
    //
    // Reference layouts: hud/zone_flash.rs:52-85 (centred banner) or
    // hud/death_prompt.rs (screen-centred prompt).
    // -----------------------------------------------------------------
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
                    font_size: 22.0,
                    ..default()
                },
                TextColor(palette::STAGE_BAD),
            ));
        });
}

/// What the widget should display this frame. Pure function of inputs so
/// we can unit-test mode transitions without a real Bevy world.
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
    server: Option<(u16, bool, f64)>, // (seconds, shutdown, anchor_secs)
    optimistic: OptimisticState,
) -> DisplayMode {
    // Server-authoritative wins whenever it's present.
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
    // No server data — fall through to optimistic / blocked.
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
    // Read snapshot.logout_countdown via `Res<SceneState>` and emit
    // toasts via a write-only event — the dedicated `drain_toast_events`
    // system folds them into `local_toasts` in PostUpdate so this
    // system stays parallel-eligible.
    scene_state: Res<SceneState>,
    mut toasts: MessageWriter<crate::snapshot::ToastEvent>,
    mut banner_q: Query<&mut Node, With<LogoutCountdownBanner>>,
    mut label_q: Query<&mut Text, With<LogoutCountdownLabel>>,
) {
    let now = time.elapsed_secs_f64();

    // Seed optimistic state on any new `LogoutRequested` message. Multiple
    // requests in a single frame just take the latest — pressing /logout
    // twice quickly should reset the optimistic clock, not stack timers.
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

    // Refresh local anchor whenever the server's seconds value changes.
    let countdown = scene_state.snapshot.logout_countdown;
    match countdown {
        Some(c) if anchor.server_seconds != Some(c.seconds_remaining) => {
            anchor.server_seconds = Some(c.seconds_remaining);
            anchor.shutdown = c.shutdown;
            anchor.anchor_secs = now;
            // Server has confirmed — clear any pending optimistic timer
            // so the watchdog can't trip into Blocked after the fact.
            if matches!(optimistic.state, OptimisticState::Optimistic { .. }) {
                optimistic.state = OptimisticState::None;
            }
        }
        None => {
            anchor.server_seconds = None;
        }
        _ => {}
    }

    // Watchdog: optimistic without a server ack within the timeout flips
    // to Blocked and emits a chat toast. This is the silent-rejection
    // surfacing — the only feedback the user gets when the server's
    // 0x0E7 validator drops the packet on the floor. We dump the
    // client-visible blocker state (open dialog, status icons) into
    // the toast so the operator doesn't have to guess which of the
    // four `blockedBy` validator states fired.
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
        // Server says 25s, optimistic says we just started — server's
        // value should drive the display.
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
        // 5s in → ~25s remaining
        let mode = compute_display(5.0, None, opt);
        assert_eq!(
            mode,
            DisplayMode::Counting {
                seconds: 25,
                shutdown: false
            }
        );
        // 2.5s in → ~28s remaining (rounded)
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
        // 2s in → still showing the banner
        assert_eq!(
            compute_display(2.0, None, blocked),
            DisplayMode::Blocked { shutdown: false }
        );
        // Past BLOCKED_DISPLAY_SECS → hidden
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
