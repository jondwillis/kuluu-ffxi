#![cfg(feature = "enhanced-death-countdown")]
//! Enhanced (non-retail) numeric KO countdown line on the death prompt.
//!
//! Retail surfaces the home-point menu and no visible clock, per the dated
//! observation in `.agents/skills/retail-observe/references/death-ko-behavior.md`.
//! The XIClient decompile parks the server's death deadline in zone state
//! (research/XIClient/src/XIClient/include/Game/State/GC_ZONE.h `field_40D6C`) and leaves its one
//! read site unimplemented, so it corroborates nothing either way. This readout
//! is addon-style and opt-in; the timer it renders is the real server value.

use bevy::prelude::*;

use crate::hud::death_prompt::is_dead;
use crate::hud::style::{self, theme};
use crate::snapshot::SceneState;

#[derive(Component)]
pub struct DeathCountdownText;

const COUNTDOWN_FONT_PX: f32 = 13.0;

const SECS_PER_MIN: u32 = 60;

pub fn countdown_bundle() -> impl Bundle {
    (
        DeathCountdownText,
        Text::new(String::new()),
        style::text_font(COUNTDOWN_FONT_PX),
        TextColor(theme::DANGER),
    )
}

fn format_mmss(secs: u32) -> String {
    format!("{}:{:02}", secs / SECS_PER_MIN, secs % SECS_PER_MIN)
}

/// The server only re-sends 0x037 char_status on status changes, not every
/// second, so the KO countdown is anchored to the last server value and ticked
/// down locally.
#[derive(Default)]
pub struct DeathCountdownAnchor {
    server_secs: Option<u32>,
    anchor_elapsed: f64,
}

pub fn update_death_countdown_system(
    time: Res<Time>,
    state: Res<SceneState>,
    mut anchor: Local<DeathCountdownAnchor>,
    mut countdown_q: Query<&mut Text, With<DeathCountdownText>>,
) {
    let now = time.elapsed_secs_f64();
    let server = is_dead(&state)
        .then_some(state.snapshot.death_homepoint_secs)
        .flatten();

    if anchor.server_secs != server {
        anchor.server_secs = server;
        anchor.anchor_elapsed = now;
    }

    if let Ok(mut text) = countdown_q.single_mut() {
        let label = match anchor.server_secs {
            Some(secs) => {
                let ticked = (now - anchor.anchor_elapsed).max(0.0) as u32;
                format!("Home Point in {}", format_mmss(secs.saturating_sub(ticked)))
            }
            None => String::new(),
        };
        if **text != label {
            **text = label;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmss_zero_pads_the_seconds_field() {
        assert_eq!(format_mmss(0), "0:00");
        assert_eq!(format_mmss(9), "0:09");
        assert_eq!(format_mmss(60), "1:00");
        assert_eq!(format_mmss(3599), "59:59");
        assert_eq!(format_mmss(3600), "60:00");
    }
}
